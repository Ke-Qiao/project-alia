use super::*;
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn accept(listener: &TcpListener) -> (TcpStream, String) {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(socket.read_u8().await.unwrap());
        assert!(head.len() < 16384);
    }
    let head = String::from_utf8(head).unwrap();
    let len = head
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    assert!(len < 65536);
    let mut body = vec![0; len];
    socket.read_exact(&mut body).await.unwrap();
    (
        socket,
        format!("{head}{}", String::from_utf8(body).unwrap()),
    )
}

fn model(api: Api, base_url: String) -> LlmService {
    let config = Config {
        provider_id: "test".into(),
        api,
        api_key: "before-key".into(),
        base_url,
        model: "before-model".into(),
        request: Default::default(),
        params: Default::default(),
        probe: false,
    };
    let native = build_model(&config).unwrap();
    LlmService {
        name: "fixture",
        profile_id: "major".into(),
        state: RwLock::new(State {
            config: Some(config),
            model: Some(native),
            generation: 0,
        }),
        stopped: AtomicBool::new(false),
        lifecycle: None,
    }
}

async fn exchange(
    api: Api,
    body: Vec<u8>,
    streaming: bool,
    chunk: usize,
    end: bool,
) -> Result<CompletionResponse> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm = model(api, format!("http://{}", listener.local_addr().unwrap()));
    let request = async {
        let request = CompletionRequest::new(Message::user("hello"));
        if !streaming {
            return llm.completion(request).await;
        }
        let mut stream = llm.completion_stream(request).await?;
        while let Some(item) = stream.next().await {
            item?;
        }
        let response = stream.finish().await?;
        validate_response(&response)?;
        Ok(response)
    };
    let peer = async {
        let (mut socket, request) = accept(&listener).await;
        assert!(request.contains("before-key"));
        let mime = if streaming {
            "text/event-stream"
        } else {
            "application/json"
        };
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        for piece in body.chunks(chunk) {
            if socket
                .write_all(format!("{:x}\r\n", piece.len()).as_bytes())
                .await
                .is_err()
                || socket.write_all(piece).await.is_err()
                || socket.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        if end {
            let _ = socket.write_all(b"0\r\n\r\n").await;
        } else {
            let mut byte = [0];
            match socket.read(&mut byte).await {
                Ok(0) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                other => panic!("response must be cancelled before peer EOF: {other:?}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let (result, ()) = tokio::join!(request, peer);
        result
    })
    .await
    .expect("local HTTP fixture deadline")
}

fn sse(value: Value) -> String {
    format!("data: {value}\n\n")
}
fn chat_delta(text: &str, reason: Value) -> String {
    sse(
        json!({"id":"r1","model":"fixture","choices":[{"index":0,"delta":{"content":text},"finish_reason":reason}]}),
    )
}
fn anthropic_event(kind: &str, mut value: Value) -> String {
    value["type"] = kind.into();
    format!("event: {kind}\ndata: {value}\n\n")
}
fn anthropic_stream() -> String {
    anthropic_event(
        "message_start",
        json!({"message":{"id":"r1","role":"assistant","model":"fixture","content":[],"usage":{"input_tokens":1,"output_tokens":0}}}),
    ) + &anthropic_event(
        "content_block_start",
        json!({"index":0,"content_block":{"type":"text","text":""}}),
    ) + &anthropic_event(
        "content_block_delta",
        json!({"index":0,"delta":{"type":"text_delta","text":"你好🌍"}}),
    ) + &anthropic_event("content_block_stop", json!({"index":0}))
        + &anthropic_event(
            "message_delta",
            json!({"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
        )
}

#[tokio::test]
async fn strict_streams_preserve_unicode_and_require_provider_terminal() {
    let complete = chat_delta("你好🌍", json!("stop")) + "data: [DONE]\n\n";
    assert_eq!(
        exchange(Api::Chat, complete.into_bytes(), true, 1, true)
            .await
            .unwrap()
            .text(),
        "你好🌍"
    );
    assert!(
        exchange(
            Api::Chat,
            chat_delta("partial", json!("stop")).into_bytes(),
            true,
            17,
            true
        )
        .await
        .is_err()
    );
    let anth = anthropic_stream();
    assert!(
        exchange(
            Api::Anthropic {
                version: "2023-06-01".into()
            },
            anth.clone().into_bytes(),
            true,
            11,
            true
        )
        .await
        .is_err()
    );
    let complete = anth + &anthropic_event("message_stop", json!({}));
    assert_eq!(
        exchange(
            Api::Anthropic {
                version: "2023-06-01".into()
            },
            complete.into_bytes(),
            true,
            1,
            true
        )
        .await
        .unwrap()
        .text(),
        "你好🌍"
    );
    let gem = sse(
        json!({"candidates":[{"content":{"role":"model","parts":[{"text":"你好🌍"}]},"finishReason":"STOP"}],"modelVersion":"fixture"}),
    );
    assert_eq!(
        exchange(Api::Gemini, gem.into_bytes(), true, 1, true)
            .await
            .unwrap()
            .text(),
        "你好🌍"
    );
    assert!(
        exchange(Api::Chat, b"data: \xff\n\n".to_vec(), true, 1, true)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn invalid_tool_arguments_and_duplicate_identity_fail_before_execution() {
    for arguments in ["", "null", "[]", "\"{}\"", "{\"x\":1"] {
        let body=json!({"id":"r1","choices":[{"message":{"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"echo","arguments":arguments}}]},"finish_reason":"tool_calls"}]}).to_string();
        assert!(
            exchange(Api::Chat, body.into_bytes(), false, 4096, true)
                .await
                .is_err(),
            "accepted {arguments}"
        );
    }
    for id in ["", "same"] {
        let calls = (0..2)
            .map(|_| json!({"id":id,"type":"function","function":{"name":"echo","arguments":"{}"}}))
            .collect::<Vec<_>>();
        let body=json!({"id":"r1","choices":[{"message":{"role":"assistant","tool_calls":calls},"finish_reason":"tool_calls"}]}).to_string();
        assert!(
            exchange(Api::Chat, body.into_bytes(), false, 4096, true)
                .await
                .is_err()
        );
    }
    let body=json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"echo","args":null}}]},"finishReason":"STOP"}]}).to_string();
    assert!(
        exchange(Api::Gemini, body.into_bytes(), false, 4096, true)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn budgets_close_unfinished_streams_and_unary_bodies() {
    for api in [
        Api::Chat,
        Api::Responses,
        Api::Anthropic {
            version: "2023-06-01".into(),
        },
        Api::Gemini,
    ] {
        assert!(
            exchange(
                api.clone(),
                vec![b' '; 4 * 1024 * 1024 + 1],
                false,
                4096,
                false
            )
            .await
            .is_err()
        );
        assert!(
            exchange(
                api,
                format!("data: {}", "x".repeat(256 * 1024)).into_bytes(),
                true,
                4096,
                false
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn stalled_requests_release_settings_lock_and_cancel_transport() {
    for api in [
        Api::Chat,
        Api::Responses,
        Api::Anthropic {
            version: "2023-06-01".into(),
        },
        Api::Gemini,
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let llm = model(api, format!("http://{}", listener.local_addr().unwrap()));
        let mut generation = Box::pin(llm.text(&[]));
        let (mut socket,request)=tokio::time::timeout(Duration::from_secs(3),async { tokio::select! { result=&mut generation=>panic!("early result {result:?}"),request=accept(&listener)=>request } }).await.unwrap();
        assert!(request.contains("before-key") && request.contains("before-model"));
        let updates = BTreeMap::from([
            ("core.profiles.llm.major.model".into(), json!("after-model")),
            ("core.providers.llm.test.api_key".into(), json!("after-key")),
        ]);
        tokio::time::timeout(Duration::from_secs(1), llm.apply_settings(&updates))
            .await
            .unwrap()
            .unwrap();
        drop(generation);
        let mut byte = [0];
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
            .await
            .unwrap();
        assert!(matches!(closed, Ok(0)) || closed.is_err());
    }
}

#[tokio::test]
async fn responses_and_unary_providers_keep_native_calls_without_raw_payloads() {
    let response = json!({"id":"r1","status":"completed","output":[{"type":"message","id":"m1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hello"}]}]});
    let streamed = sse(json!({"type":"response.completed","response":response}));
    let result = exchange(Api::Responses, streamed.into_bytes(), true, 11, true)
        .await
        .unwrap();
    assert_eq!(result.text(), "hello");
    assert!(result.raw.is_null());
    assert_eq!(
        exchange(
            Api::Responses,
            response.to_string().into_bytes(),
            false,
            17,
            true
        )
        .await
        .unwrap()
        .text(),
        "hello"
    );
    let anthropic = json!({"id":"r1","type":"message","role":"assistant","model":"fixture","content":[{"type":"tool_use","id":"c1","name":"echo","input":{"value":1}}],"stop_reason":"tool_use","usage":{"input_tokens":1,"output_tokens":1}});
    let result = exchange(
        Api::Anthropic {
            version: "2023-06-01".into(),
        },
        anthropic.to_string().into_bytes(),
        false,
        17,
        true,
    )
    .await
    .unwrap();
    assert!(
        matches!(result.choice.as_slice(),[AssistantContent::ToolCall(call)] if call.function.arguments["value"]==1)
    );
    let gemini=json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"echo","args":{"value":1}}}]},"finishReason":"STOP"}]}).to_string();
    let result = exchange(Api::Gemini, gemini.into_bytes(), false, 17, true)
        .await
        .unwrap();
    assert!(
        matches!(result.choice.as_slice(),[AssistantContent::ToolCall(call)] if call.function.arguments["value"]==1)
    );
    for id in ["", "ok"] {
        let response=json!({"id":"r1","status":"completed","output":[{"type":"function_call","call_id":id,"name":"echo","arguments":if id.is_empty(){"{}"}else{"null"}}]}).to_string();
        assert!(
            exchange(Api::Responses, response.into_bytes(), false, 17, true)
                .await
                .is_err()
        );
    }
}

async fn configured_request_body(api: Api, options: LlmRequestOptionsSection) -> Value {
    let response = match &api {
        Api::Chat => {
            json!({"id":"r1","model":"fixture","choices":[{"index":0,"message":{"role":"assistant","content":"{}"},"finish_reason":"stop"}]})
        }
        Api::Responses => {
            json!({"id":"r1","object":"response","model":"fixture","status":"completed","output":[{"type":"message","id":"m1","role":"assistant","status":"completed","content":[{"type":"output_text","text":"{}","annotations":[]}]}]})
        }
        Api::Gemini => {
            json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"{}"}]},"finishReason":"STOP"}]})
        }
        Api::Anthropic { .. } => unreachable!(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm = model(api, format!("http://{}", listener.local_addr().unwrap()));
    llm.state.write().await.config.as_mut().unwrap().request = options;
    let messages = [ChatMessage::new("user", "Return a JSON object.")];
    let request = llm.text(&messages);
    let peer = async {
        let (mut socket, request) = accept(&listener).await;
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        let response = response.to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
        body
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        let (result, body) = tokio::join!(request, peer);
        assert_eq!(result.unwrap(), "{}");
        body
    })
    .await
    .expect("request format fixture deadline")
}

#[tokio::test]
async fn profile_json_modes_and_schema_names_reach_each_native_protocol() {
    let options = LlmRequestOptionsSection {
        json_mode: true,
        ..Default::default()
    };
    let chat = configured_request_body(Api::Chat, options.clone()).await;
    assert_eq!(chat["response_format"], json!({"type":"json_object"}));
    let responses = configured_request_body(Api::Responses, options.clone()).await;
    assert_eq!(responses["text"]["format"], json!({"type":"json_object"}));
    let gemini = configured_request_body(Api::Gemini, options).await;
    assert_eq!(
        gemini["generationConfig"]["responseMimeType"],
        "application/json"
    );
    for api in [Api::Chat, Api::Responses] {
        let is_chat = matches!(api, Api::Chat);
        let body = configured_request_body(api, LlmRequestOptionsSection {
            json_schema_name: Some("profile_answer".into()),
            json_schema: Some(json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false})),
            ..Default::default()
        }).await;
        let schema = if is_chat {
            &body["response_format"]["json_schema"]
        } else {
            &body["text"]["format"]
        };
        assert_eq!(schema["name"], "profile_answer");
        assert_eq!(schema["schema"]["properties"]["answer"]["type"], "string");
    }
}
