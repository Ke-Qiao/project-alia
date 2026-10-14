use std::collections::BTreeMap;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::providers::adapter::LlmAdapter;

async fn accept_request(listener: &TcpListener) -> (TcpStream, String) {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        bytes.push(socket.read_u8().await.unwrap());
        assert!(bytes.len() <= 8192, "bounded request headers");
    }
    let headers = String::from_utf8(bytes).unwrap();
    let body_len = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    assert!(body_len <= 16384, "bounded local request body");
    let mut body = vec![0; body_len];
    socket.read_exact(&mut body).await.unwrap();
    (
        socket,
        format!("{headers}{}", String::from_utf8(body).unwrap()),
    )
}

pub(super) async fn assert_stalled_generation_releases_settings(
    llm: &dyn LlmAdapter,
    listener: &TcpListener,
    provider: &str,
) {
    let model_key = "core.profiles.llm.major.model";
    let api_key = format!("core.providers.llm.{provider}.api_key");
    let mut model = "before-model".to_string();
    let mut key = "before-key".to_string();
    llm.apply_settings(&BTreeMap::from([
        (model_key.into(), json!(model)),
        (api_key.clone(), json!(key)),
    ]))
    .await
    .unwrap();

    // Cover each public entry once, with headers, successful body and error body stalls.
    for mode in 0..4 {
        let mut generation = Box::pin(async {
            match mode {
                0 => llm.chat(&[]).await.map(|_| ()),
                1 => llm.chat_with_tools(&[], &[]).await.map(|_| ()),
                2 => {
                    let mut stream = llm.chat_stream(&[]).await?;
                    while let Some(chunk) = stream.next().await {
                        chunk?;
                    }
                    Ok(())
                }
                _ => llm.chat_stream_with_tools(&[], &[]).await.map(|_| ()),
            }
        });
        let (mut socket, request) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = &mut generation => panic!("generation ended before request: {result:?}"),
                request = accept_request(listener) => request,
            }
        })
        .await
        .expect("local generation request must arrive");
        assert!(request.contains(&model), "request uses one model snapshot");
        assert!(
            request.contains(&key),
            "request uses one credential snapshot"
        );

        if mode != 2 {
            let status = if mode == 1 {
                "503 Service Unavailable"
            } else {
                "200 OK"
            };
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n{{"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        model = format!("after-model-{mode}");
        key = format!("after-key-{mode}");
        let updates = BTreeMap::from([
            (model_key.into(), json!(model)),
            (api_key.clone(), json!(key)),
        ]);
        tokio::time::timeout(Duration::from_secs(1), async {
            tokio::select! {
                biased;
                result = &mut generation => panic!("generation ended during peer stall: {result:?}"),
                applied = llm.apply_settings(&updates) => assert_eq!(applied.unwrap().len(), 2),
            }
        })
        .await
        .expect("stalled generation must not retain the settings read lock");

        drop(generation);
        let mut byte = [0; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
            .await
            .expect("dropping the owned generation must close its incomplete response");
        match closed {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("generation connection still active: {other:?}"),
        }
    }
    assert_eq!(
        llm.llm_capabilities().await.provider_specific["model"],
        json!(model)
    );
}
