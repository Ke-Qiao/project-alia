use super::*;
use crate::module::lifecycle::ModuleHealth;
use std::io::{ErrorKind, Read, Write};

fn packet(kind: u8, id: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xa5, 1, kind];
    bytes.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes.push(0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))));
    bytes
}

async fn verify_board(master: &mut std::fs::File, wake: &dyn WakeWordEngine) {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut pending = Vec::new();
        while wake.health_check().await != ModuleHealth::Healthy {
            let mut bytes = [0u8; 256];
            match master.read(&mut bytes) {
                Ok(count) => pending.extend_from_slice(&bytes[..count]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                result => panic!("reading fixture UART query: {result:?}"),
            }
            while pending.len() >= 7 {
                assert_eq!(&pending[..3], &[0xa5, 1, 5]);
                let length = u16::from_le_bytes([pending[3], pending[4]]) as usize + 8;
                assert!(length <= 256);
                if pending.len() < length {
                    break;
                }
                let id = u16::from_le_bytes([pending[5], pending[6]]);
                master
                    .write_all(&packet(
                        5,
                        id,
                        br#"{"code":0,"content":"version: UART_REAL, circle6:1.9.6.fixture"}"#,
                    ))
                    .unwrap();
                pending.drain(..length);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn real_r1_pty_owner_delivers_once_through_speech_and_authenticated_voice() {
    use futures::FutureExt;
    let Harness {
        mut runtime,
        audio,
        events,
        local,
        mut seen,
        shutdown,
        mut server,
        _directory,
    } = harness(8).await;
    let endpoint = runtime.binding.status().await.server_endpoint.unwrap();
    runtime.binding.unbind("test").await.unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&rand::random());
    let public = key.verifying_key().to_bytes();
    let issued = alia_transport::current_unix_timestamp().unwrap();
    let payload = alia_transport::bind_payload("r1-fixture", "Fixture", &endpoint, &public, issued);
    runtime
        .binding
        .bind(
            "r1-fixture",
            "Fixture",
            &endpoint,
            &public,
            &alia_transport::sign_payload(&key, &payload),
            issued,
        )
        .await
        .unwrap();
    let (mut master, wake) = crate::module::r1::test_pty_engine();
    let wake = Arc::new(wake);
    runtime.wakeword = wake.clone();
    let cancel = runtime.cancel.clone();
    let mut lifecycle = events.subscribe();
    let mut task = tokio::spawn(runtime.run());
    let result = std::panic::AssertUnwindSafe(async {
        // Establish that speech startup reset has run before answering its UART query.
        publish(&audio, 0, 0);
        reached(&local.captured, 1).await;
        verify_board(&mut master, wake.as_ref()).await;
        assert!(seen.try_recv().is_err());
        let payload = br#"{"type":"aiui_event","content":{"info":"{\"ivw\":{\"keyword\":\"xiao3-fei1-xiao3-fei1\",\"angle\":359.999999,\"physical\":0}}"}}"#;
        let bytes = packet(4, 0x1104, payload);
        master.write_all(&bytes[..6]).unwrap();
        master.write_all(&bytes[6..]).unwrap();
        let mut offset = 320;
        let event = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                publish(&audio, 7, offset);
                offset += 320;
                tokio::select! {
                    event = lifecycle.recv() => if let ModuleEvent::WakeWordDetected(wake) = event.unwrap() { break wake; },
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {},
                }
            }
        }).await.unwrap();
        assert_eq!(event.keyword, "xiao3-fei1-xiao3-fei1");
        assert!(event.timestamp_us > 0);
        assert_eq!(event.source, WakeWordSource::R1 {
            message_id: 0x1104, angle_deg: Some(359.999999), beam: Some(0),
        });
        let Seen::Opened { output, .. } = received(&mut seen).await else {
            panic!("real UART owner must reach signed voice Open")
        };
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        master.write_all(&bytes).unwrap();
        publish(&audio, 7, offset);
        assert!(matches!(received(&mut seen).await, Seen::Audio(_)));
        assert!(seen.try_recv().is_err(), "UART replay opened a second voice session");
        assert!(!std::iter::from_fn(|| lifecycle.try_recv().ok())
            .any(|event| matches!(event, ModuleEvent::WakeWordDetected(_))));
        drop(output);
    }).catch_unwind().await;
    cancel.cancel();
    let drainer = tokio::spawn(async move { while seen.recv().await.is_some() {} });
    let runtime_result = tokio::time::timeout(Duration::from_secs(3), &mut task).await;
    if runtime_result.is_err() {
        task.abort();
        let _ = task.await;
    }
    let wake_result = wake.stop().await;
    drop(master);
    shutdown.cancel();
    let server_result = tokio::time::timeout(Duration::from_secs(3), &mut server).await;
    if server_result.is_err() {
        server.abort();
        let _ = server.await;
    }
    drainer.abort();
    let _ = drainer.await;
    runtime_result.unwrap().unwrap();
    wake_result.unwrap();
    server_result.unwrap().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
