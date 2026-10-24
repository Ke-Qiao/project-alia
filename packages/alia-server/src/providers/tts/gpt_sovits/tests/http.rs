//! Actual HTTP framing and body lifetime; no database or live TTS model.
use super::*;
use crate::modules::delivery::audio::MAX_RESPONSE_ENCODED_BYTES;
use axum::{
    Router,
    body::{Body, Bytes},
    http::Response,
    routing::post,
};
use futures::{StreamExt, stream};
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{net::TcpListener, sync::Notify, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn remote_weights_require_both_acknowledgements_and_retry_after_partial_failure() {
    use axum::{
        extract::{Path, Query},
        http::StatusCode,
        routing::get,
    };
    use tokio::sync::Mutex;

    for reject_sovits in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider = GptSovitsProviderSection {
            endpoint: format!("http://{}/tts", listener.local_addr().unwrap()),
            request_timeout_sec: 2,
            ..Default::default()
        };
        let voices = BTreeMap::from([(
            "test-voice".into(),
            VoiceProfileSection {
                provider_specific: BTreeMap::from([
                    (
                        "gpt_weights_path".into(),
                        serde_json::json!("mock-gpt.ckpt"),
                    ),
                    (
                        "sovits_weights_path".into(),
                        serde_json::json!("mock-sovits.pth"),
                    ),
                ]),
                ..Default::default()
            },
        )]);
        let mut tts = GptSovitsTts::new(&provider, "test-voice", &voices, &BTreeMap::new());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );

        let reject = Arc::new(std::sync::atomic::AtomicBool::new(reject_sovits));
        let remote_reject = reject.clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let router = Router::new().route(
            "/{path}",
            get(
                move |Path(path): Path<String>, Query(query): Query<BTreeMap<String, String>>| {
                    let recorded = recorded.clone();
                    let remote_reject = remote_reject.clone();
                    async move {
                        if path == "tts" {
                            return StatusCode::BAD_REQUEST;
                        }
                        let reject =
                            remote_reject.load(Ordering::SeqCst) && path == "set_sovits_weights";
                        recorded
                            .lock()
                            .await
                            .push((path, query["weights_path"].clone()));
                        if reject {
                            StatusCode::INTERNAL_SERVER_ERROR
                        } else {
                            StatusCode::OK
                        }
                    }
                },
            ),
        );
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        assert!(tts.synthesize_request(&request(), budget()).await.is_err());
        let initialized = tokio::time::timeout(Duration::from_secs(5), tts.init())
            .await
            .unwrap();
        assert_eq!(initialized.is_err(), reject_sovits);
        assert_eq!(
            *calls.lock().await,
            vec![
                ("set_gpt_weights".to_string(), "mock-gpt.ckpt".to_string()),
                (
                    "set_sovits_weights".to_string(),
                    "mock-sovits.pth".to_string()
                ),
            ]
        );
        if reject_sovits {
            assert_eq!(
                tts.config.read().await.weight_state,
                WeightState::Unconfirmed {
                    gpt_acknowledged: true,
                    sovits_acknowledged: false,
                }
            );
            assert!(matches!(
                tts.health_check().await,
                ModuleHealth::Unhealthy(_)
            ));
            assert!(tts.synthesize_request(&request(), budget()).await.is_err());
            assert!(
                tts.synthesize_stream_request(&request(), budget())
                    .await
                    .is_err()
            );
            reject.store(false, Ordering::SeqCst);
            tts.apply_settings(&BTreeMap::from([(
                "core.profiles.voice.default.provider_specific.sovits_weights_path".into(),
                serde_json::json!("mock-sovits.pth"),
            )]))
            .await
            .unwrap();
            assert_eq!(
                calls.lock().await.len(),
                4,
                "retry must set both desired weights"
            );
        }
        assert_eq!(
            tts.config.read().await.weight_state,
            WeightState::Acknowledged
        );
        assert!(matches!(tts.health_check().await, ModuleHealth::Healthy));
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn settings_wait_for_the_original_audio_stream_to_release_weights() {
    let peer = Peer::new(|| Response::new(chunks(Bytes::from_static(b"audio"), 1))).await;
    let input = peer
        .tts
        .synthesize_stream_request(&request(), budget())
        .await
        .unwrap();
    let updates = BTreeMap::from([(
        "core.providers.gpt_sovits.endpoint".into(),
        serde_json::json!("http://127.0.0.1:9/tts"),
    )]);
    let mut update = Box::pin(peer.tts.apply_settings(&updates));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut update)
            .await
            .is_err()
    );
    drop(input);
    tokio::time::timeout(Duration::from_secs(2), update)
        .await
        .unwrap()
        .unwrap();
    peer.close().await;
}

struct Peer {
    tts: Arc<GptSovitsTts>,
    seen: Arc<Notify>,
    calls: Arc<AtomicUsize>,
    stop: CancellationToken,
    task: Option<JoinHandle<()>>,
}
impl Peer {
    async fn new(reply: impl Fn() -> Response<Body> + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/tts", listener.local_addr().unwrap());
        let seen = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let signal = seen.clone();
        let count = calls.clone();
        let reply = Arc::new(reply);
        let stop = CancellationToken::new();
        let body_stop = stop.clone();
        let router = Router::new().route(
            "/tts",
            // Consume the real request body before sending a stalled response.
            post(move |_: Bytes| {
                let reply = reply.clone();
                let signal = signal.clone();
                let count = count.clone();
                let body_stop = body_stop.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    signal.notify_one();
                    // Graceful server shutdown does not finish an infinite body.
                    // Only fixture teardown releases these deliberately stalled peers.
                    reply().map(|body| {
                        Body::from_stream(
                            body.into_data_stream()
                                .take_until(body_stop.cancelled_owned()),
                        )
                    })
                }
            }),
        );
        let shutdown = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        let tts = Arc::new(GptSovitsTts::new(
            &GptSovitsProviderSection {
                endpoint,
                request_timeout_sec: 15,
                stream_idle_timeout_sec: 1,
                ..Default::default()
            },
            "default",
            &BTreeMap::new(),
            &BTreeMap::new(),
        ));
        Self {
            tts,
            seen,
            calls,
            stop,
            task: Some(task),
        }
    }
    async fn close(mut self) {
        self.stop.cancel();
        let mut task = self.task.take().unwrap();
        match tokio::time::timeout(Duration::from_secs(3), &mut task).await {
            Ok(result) => result.unwrap(),
            Err(error) => {
                task.abort();
                let stopped = task.await;
                panic!("HTTP peer shutdown timed out: {error}; aborted task: {stopped:?}");
            }
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
fn budget() -> ResponseAudioBudget {
    ResponseAudioBudget::for_response(uuid::Uuid::new_v4()).unwrap()
}
fn request() -> TtsRequest {
    TtsRequest::plain("bounded speech")
}
fn chunks(chunk: Bytes, count: usize) -> Body {
    Body::from_stream(stream::iter(
        (0..count).map(move |_| Ok::<_, Infallible>(chunk.clone())),
    ))
}
fn wav() -> Vec<u8> {
    let mut wav = minimal_wav_header(24_000);
    wav[4..8].copy_from_slice(&44_u32.to_le_bytes());
    wav[40..44].copy_from_slice(&8_u32.to_le_bytes());
    wav.extend_from_slice(&[1, 0, 2, 0, 3, 0, 4, 0]);
    wav
}

#[tokio::test]
async fn oversized_content_length_rejects_without_consuming_a_body() {
    for streaming in [false, true] {
        let limit = if streaming {
            MAX_TTS_STREAM_BYTES
        } else {
            MAX_ENCODED_PAYLOAD_BYTES
        };
        let peer = Peer::new(move || {
            Response::builder()
                .header("content-length", limit + 1)
                .body(Body::from_stream(stream::pending::<
                    Result<Bytes, Infallible>,
                >()))
                .unwrap()
        })
        .await;
        let budget = budget();
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            if streaming {
                peer.tts
                    .synthesize_stream_request(&request(), budget.clone())
                    .await
                    .map(|_| ())
            } else {
                peer.tts
                    .synthesize_request(&request(), budget.clone())
                    .await
                    .map(|_| ())
            }
        })
        .await
        .unwrap();
        assert!(result.unwrap_err().to_string().contains("Content-Length"));
        assert_eq!(budget.reserved_bytes(), 0);
        assert_eq!(peer.calls.load(Ordering::SeqCst), 1);
        peer.close().await;
    }
}

#[tokio::test]
async fn unknown_length_buffered_body_rejects_cumulative_overflow() {
    let peer = Peer::new(|| Response::new(chunks(Bytes::from(vec![1; 16 * 1024]), 65))).await;
    let budget = budget();
    let error = peer
        .tts
        .synthesize_request(&request(), budget.clone())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("body exceeds 1048576"),
        "{error}"
    );
    assert_eq!(budget.reserved_bytes(), 0);
    peer.close().await;
}

#[tokio::test]
async fn unending_chunked_stream_has_a_total_bound_even_if_consumer_drops_every_chunk() {
    let peer = Peer::new(|| {
        Response::new(Body::from_stream(stream::repeat(Ok::<_, Infallible>(
            Bytes::from_static(&[1; 16 * 1024]),
        ))))
    })
    .await;
    let budget = budget();
    let mut input = peer
        .tts
        .synthesize_stream_request(&request(), budget.clone())
        .await
        .unwrap();
    let mut received = 0;
    let error = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match input
                .next()
                .await
                .expect("limit must be an explicit stream error")
            {
                Ok(audio) => {
                    assert!(audio.len() <= MAX_ENCODED_PAYLOAD_BYTES);
                    received += audio.len();
                    assert!(budget.reserved_bytes() <= MAX_RESPONSE_ENCODED_BYTES);
                }
                Err(error) => break error,
            }
        }
    })
    .await
    .unwrap();
    assert!(error.to_string().contains("8 MiB cumulative"), "{error}");
    assert!(received <= MAX_TTS_STREAM_BYTES);
    assert!(received >= MAX_TTS_STREAM_BYTES - MAX_ENCODED_PAYLOAD_BYTES);
    drop(input);
    assert_eq!(budget.reserved_bytes(), 0);
    peer.close().await;
}

#[tokio::test]
async fn error_body_stops_at_four_kib_without_waiting_for_eof() {
    let peer = Peer::new(|| {
        Response::builder()
            .status(500)
            .body(Body::from_stream(
                stream::once(async { Ok::<_, Infallible>(Bytes::from(vec![b'x'; 32 * 1024])) })
                    .chain(stream::pending()),
            ))
            .unwrap()
    })
    .await;
    for streaming in [false, true] {
        let budget = budget();
        let error = tokio::time::timeout(Duration::from_secs(2), async {
            if streaming {
                peer.tts
                    .synthesize_stream_request(&request(), budget.clone())
                    .await
                    .err()
                    .unwrap()
            } else {
                peer.tts
                    .synthesize_request(&request(), budget.clone())
                    .await
                    .unwrap_err()
            }
        })
        .await
        .unwrap();
        let message = error.to_string();
        assert_eq!(message.split_once(": ").unwrap().1.len(), ERROR_BODY_BYTES);
        assert_eq!(budget.reserved_bytes(), 0);
    }
    peer.close().await;
}

#[tokio::test]
async fn cancelling_partial_body_and_dropping_unpolled_stream_release_credits() {
    let peer = Peer::new(|| {
        Response::new(Body::from_stream(
            stream::once(async { Ok::<_, Infallible>(Bytes::from_static(&[1; 32])) })
                .chain(stream::pending()),
        ))
    })
    .await;
    let budget = budget();
    let tts = peer.tts.clone();
    let owned = budget.clone();
    let task = tokio::spawn(async move { tts.synthesize_request(&request(), owned).await });
    tokio::time::timeout(Duration::from_secs(2), peer.seen.notified())
        .await
        .unwrap();
    assert_eq!(budget.reserved_bytes(), MAX_ENCODED_PAYLOAD_BYTES);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(budget.reserved_bytes(), 0);
    let input = peer
        .tts
        .synthesize_stream_request(&request(), budget.clone())
        .await
        .unwrap();
    assert_eq!(budget.reserved_bytes(), MAX_ENCODED_PAYLOAD_BYTES);
    drop(input);
    assert_eq!(budget.reserved_bytes(), 0);
    peer.close().await;
}

#[tokio::test]
async fn dropped_http_stream_does_not_release_a_delivered_clone() {
    let peer = Peer::new(|| {
        Response::new(Body::from_stream(
            stream::once(async { Ok::<_, Infallible>(Bytes::from_static(&[1; 32])) })
                .chain(stream::pending()),
        ))
    })
    .await;
    let budget = budget();
    let mut input = peer
        .tts
        .synthesize_stream_request(&request(), budget.clone())
        .await
        .unwrap();
    let audio = input.next().await.unwrap().unwrap();
    let length = audio.len();
    assert_eq!(length, 32);
    let clone = audio.clone().into_bytes();
    drop((input, audio));
    assert_eq!(budget.reserved_bytes(), length);
    drop(clone);
    assert_eq!(budget.reserved_bytes(), 0);
    peer.close().await;
}

#[tokio::test]
async fn small_wav_buffered_and_fragmented_pitch_stream_keep_bytes_and_owner() {
    let source = wav();
    let served = source.clone();
    let peer = Peer::new(move || {
        Response::new(Body::from_stream(stream::iter(
            served
                .chunks(7)
                .map(|p| Ok::<_, Infallible>(Bytes::copy_from_slice(p)))
                .collect::<Vec<_>>(),
        )))
    })
    .await;
    let budget = budget();
    let audio = peer
        .tts
        .synthesize_request(&request(), budget.clone())
        .await
        .unwrap();
    assert_eq!(&*audio, &source);
    assert_eq!(audio.owner(), budget.owner());
    let pcm = crate::modules::interaction::streaming::tts_bytes_to_pcm_payload(audio.into_buffer())
        .unwrap();
    assert_eq!(&*pcm.data, &source[44..]);
    assert_eq!(budget.reserved_bytes(), source.len());
    drop(pcm);
    assert_eq!(budget.reserved_bytes(), 0);
    peer.tts
        .config
        .write()
        .await
        .sample_rate_pitch_fallback_enabled = true;
    let mut pitch_request = request();
    pitch_request.prosody.pitch_shift = 0.5;
    let mut input = peer
        .tts
        .synthesize_stream_request(&pitch_request, budget.clone())
        .await
        .unwrap();
    let mut rewritten = Vec::new();
    while let Some(audio) = input.next().await {
        let audio = audio.unwrap();
        assert_eq!(audio.owner(), budget.owner());
        rewritten.extend_from_slice(&audio);
    }
    drop(input);
    assert_eq!(rewritten.len(), source.len());
    assert_eq!(&rewritten[36..], &source[36..]);
    let rate = u32::from_le_bytes(rewritten[24..28].try_into().unwrap());
    assert!(rate > 24_000);
    assert_eq!(
        u32::from_le_bytes(rewritten[28..32].try_into().unwrap()),
        rate * 2
    );
    assert_eq!(budget.reserved_bytes(), 0);
    peer.close().await;
}

#[tokio::test]
async fn incomplete_pitch_header_and_idle_timeout_are_errors_not_normal_eof() {
    let peer = Peer::new(|| Response::new(chunks(Bytes::from_static(b"RIFF1234WAVE"), 1))).await;
    peer.tts
        .config
        .write()
        .await
        .sample_rate_pitch_fallback_enabled = true;
    let budget = budget();
    let mut request = request();
    request.prosody.pitch_shift = 0.5;
    let mut input = peer
        .tts
        .synthesize_stream_request(&request, budget.clone())
        .await
        .unwrap();
    assert!(
        input
            .next()
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("inside pitch WAV header")
    );
    drop(input);
    assert_eq!(budget.reserved_bytes(), 0);
    peer.close().await;
    let peer = Peer::new(|| {
        Response::new(Body::from_stream(stream::pending::<
            Result<Bytes, Infallible>,
        >()))
    })
    .await;
    let mut input = peer
        .tts
        .synthesize_stream_request(&request, budget.clone())
        .await
        .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(3), input.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("idle timeout"));
    drop(input);
    assert_eq!(budget.reserved_bytes(), 0);
    peer.close().await;
}

#[test]
fn pitch_does_not_rewrite_an_unproven_fixed_offset_or_accept_partial_header() {
    let mut config = test_config();
    config.sample_rate_pitch_fallback_enabled = true;
    let mut request = request();
    request.prosody.pitch_shift = 0.5;
    let (_, prosody) = config.request(&request, 1);
    let mut source = wav();
    source[12..16].copy_from_slice(b"JUNK");
    let original = source.clone();
    assert!(apply_pitch_shift_to_wav(&mut source, prosody).is_err());
    assert_eq!(source, original);
    let mut pitch = StreamingWavPitchRewriter::new(prosody);
    assert_eq!(pitch.push(&source[..12]).unwrap(), (12, None));
    assert!(pitch.finish().is_err());
    assert!(pitch.push(&source[12..]).is_err());
}

#[tokio::test]
async fn health_and_synthesis_preserve_unknown_state_after_cancelled_weight_switch() {
    use axum::{http::StatusCode, routing::get};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/tts", listener.local_addr().unwrap());
    let reached_sovits = Arc::new(Notify::new());
    let entered = reached_sovits.clone();
    let release_sovits = Arc::new(Notify::new());
    let release = release_sovits.clone();
    let router = Router::new()
        .route("/tts", get(|| async { StatusCode::BAD_REQUEST }))
        .route("/set_gpt_weights", get(|| async { StatusCode::OK }))
        .route(
            "/set_sovits_weights",
            get(move || {
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    entered.notify_one();
                    release.notified().await;
                    StatusCode::OK
                }
            }),
        );
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    let provider = GptSovitsProviderSection {
        endpoint,
        ..Default::default()
    };
    let tts = GptSovitsTts::new(&provider, "test", &BTreeMap::new(), &BTreeMap::new());
    let updates = BTreeMap::from([
        (
            "core.profiles.voice.default.provider_specific.gpt_weights_path".into(),
            serde_json::json!("new.ckpt"),
        ),
        (
            "core.profiles.voice.default.provider_specific.sovits_weights_path".into(),
            serde_json::json!("new.pth"),
        ),
    ]);
    {
        let switch = tts.apply_settings(&updates);
        tokio::pin!(switch);
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::select! {
                _ = reached_sovits.notified() => {},
                result = &mut switch => panic!("switch should wait for second acknowledgement: {result:?}"),
            }
        }).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(100), tts.health_check()).await.unwrap(),
            ModuleHealth::Degraded(message) if message.contains("in progress")
        ));
    }
    assert_eq!(
        tts.config.read().await.weight_state,
        WeightState::Unconfirmed {
            gpt_acknowledged: true,
            sovits_acknowledged: false
        }
    );
    assert!(matches!(
        tts.health_check().await,
        ModuleHealth::Unhealthy(_)
    ));
    assert!(tts.synthesize_request(&request(), budget()).await.is_err());
    assert!(
        tts.synthesize_stream_request(&request(), budget())
            .await
            .is_err()
    );
    release_sovits.notify_one();
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
}
