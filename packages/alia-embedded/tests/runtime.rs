use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use alia_embedded::config::{EmbeddedConfig, ModulesConfig};
use alia_embedded::module::EmbeddedBuilder;
use alia_embedded::module::context::{AudioBus, ModuleContext};
use alia_embedded::module::lifecycle::{ModuleLifecycle, ShutdownIncomplete};
use alia_embedded::module::traits::*;
use anyhow::{Result, bail};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Observed {
    events: Mutex<Vec<String>>,
    init_entered: Notify,
    start_entered: Notify,
    metadata_entered: Notify,
    stopped: Notify,
    worker_running: AtomicBool,
    port: AtomicU16,
}
impl Observed {
    fn record(&self, event: &str, tag: &str) {
        self.events.lock().unwrap().push(format!("{event}:{tag}"));
    }
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct Gate(Mutex<bool>, Condvar);
struct ReleaseGate(Arc<Gate>);
impl Drop for ReleaseGate {
    fn drop(&mut self) {
        self.0.release();
    }
}
impl Gate {
    fn wait(&self) {
        let mut released = self.0.lock().unwrap();
        while !*released {
            released = self.1.wait(released).unwrap();
        }
    }
    fn release(&self) {
        *self.0.lock().unwrap() = true;
        self.1.notify_all();
    }
}

struct Probe {
    tag: &'static str,
    seen: Arc<Observed>,
    fail_init: bool,
    fail_start: bool,
    wait_init: bool,
    wait_start: bool,
    fail_capture: bool,
    wait_metadata: bool,
    stop_gate: Option<Arc<Gate>>,
    worker: tokio::sync::Mutex<Option<(CancellationToken, tokio::task::JoinHandle<()>)>>,
}
impl Probe {
    fn new(tag: &'static str, seen: &Arc<Observed>) -> Self {
        Self {
            tag,
            seen: seen.clone(),
            fail_init: false,
            fail_start: false,
            wait_init: false,
            wait_start: false,
            fail_capture: false,
            wait_metadata: false,
            stop_gate: None,
            worker: Default::default(),
        }
    }
}
#[tonic::async_trait]
impl ModuleLifecycle for Probe {
    fn name(&self) -> &'static str {
        self.tag
    }
    async fn init(&mut self, ctx: &ModuleContext) -> Result<()> {
        self.seen.record("init", self.tag);
        self.seen.port.store(
            ctx.config.device_runtime.listen_addr.port(),
            Ordering::Release,
        );
        self.seen.init_entered.notify_one();
        if self.wait_init {
            std::future::pending::<()>().await;
        }
        if self.fail_init {
            bail!("injected init failure");
        }
        Ok(())
    }
    async fn start(&self) -> Result<()> {
        self.seen.record("start", self.tag);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let seen = self.seen.clone();
        seen.worker_running.store(true, Ordering::Release);
        let task = tokio::spawn(async move {
            stop.cancelled().await;
            seen.worker_running.store(false, Ordering::Release);
        });
        *self.worker.lock().await = Some((cancel, task));
        self.seen.start_entered.notify_one();
        if self.wait_start {
            std::future::pending::<()>().await;
        }
        if self.fail_start {
            bail!("injected start failure");
        }
        Ok(())
    }
    async fn stop(&self) -> Result<()> {
        self.seen.record("stop", self.tag);
        if let Some(gate) = &self.stop_gate {
            gate.wait();
        }
        if let Some((cancel, task)) = self.worker.lock().await.take() {
            cancel.cancel();
            task.await?;
        }
        self.seen.stopped.notify_one();
        Ok(())
    }
}
impl WakeWordEngine for Probe {
    fn reset(&self) -> Result<()> {
        Ok(())
    }

    fn detect(&self, _: &[i16]) -> Option<WakeWordEvent> {
        None
    }
}
impl AudioPlaybackBackend for Probe {
    fn play(&self, _: &[i16], _: u32) -> Result<()> {
        Ok(())
    }
    fn stop_playback(&self) -> Result<()> {
        Ok(())
    }
    fn is_available(&self) -> bool {
        true
    }
}
impl AudioCaptureBackend for Probe {
    fn start_capture(&self, _: &AudioBus) -> Result<()> {
        if self.fail_capture {
            bail!("injected microphone startup failure");
        }
        Ok(())
    }
    fn stop_capture(&self) -> Result<()> {
        Ok(())
    }
    fn is_available(&self) -> bool {
        true
    }
}
#[tonic::async_trait]
impl DiscoveryBackend for Probe {
    async fn register_service(&self, _: &DeviceInfo) -> Result<()> {
        Ok(())
    }
    async fn update_metadata(&self, _: &DiscoveryMetadata) -> Result<()> {
        if self.wait_metadata {
            self.seen.metadata_entered.notify_one();
            std::future::pending::<()>().await;
        }
        Ok(())
    }
    async fn unregister(&self) -> Result<()> {
        Ok(())
    }
}

fn config(dir: &tempfile::TempDir) -> EmbeddedConfig {
    let mut config = EmbeddedConfig::default();
    config.config_path = dir.path().join("embedded.toml");
    config.modules = ModulesConfig {
        wakeword: "noop".into(),
        edge_stt: "noop".into(),
        vad: "noop".into(),
        direct_response: "noop".into(),
        audio_capture: "noop".into(),
        audio_playback: "noop".into(),
        mixer: "noop".into(),
        face_tracker: "noop".into(),
        discovery: "noop".into(),
    };
    config.motion.enabled = false;
    config.vision.enabled = false;
    config.device_runtime.listen_addr = "127.0.0.1:0".parse().unwrap();
    config.device_runtime.shutdown_grace_ms = 1_000;
    config.device_runtime.grpc_drain_timeout_ms = 50;
    config.identity.keypair_path = dir.path().join("device.key");
    alia_embedded::identity::ensure_device_keypair(&config.identity.keypair_path).unwrap();
    config
}

#[tokio::test]
async fn factory_and_partial_init_failure_roll_back_in_reverse_order() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Observed::default());
    let mut cfg = config(&dir);
    cfg.modules.edge_stt = "missing-engine".into();
    let error = EmbeddedBuilder::from_config(cfg)
        .await
        .unwrap()
        .with_wakeword(Probe::new("wake", &seen))
        .build()
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("unknown edge_stt"));
    assert_eq!(seen.events(), ["init:wake", "stop:wake"]);

    let mut playback = Probe::new("playback", &seen);
    playback.fail_init = true;
    let error = EmbeddedBuilder::from_config(config(&dir))
        .await
        .unwrap()
        .with_wakeword(Probe::new("wake2", &seen))
        .with_audio_playback(playback)
        .build()
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("initializing module playback"));
    assert_eq!(
        &seen.events()[2..],
        ["init:wake2", "init:playback", "stop:playback", "stop:wake2"]
    );
}

#[tokio::test]
async fn partially_started_worker_is_joined_before_failed_build_returns() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Observed::default());
    let mut capture = Probe::new("capture", &seen);
    capture.fail_start = true;
    let error = EmbeddedBuilder::from_config(config(&dir))
        .await
        .unwrap()
        .with_audio_capture(capture)
        .build()
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("starting module capture"));
    assert!(!seen.worker_running.load(Ordering::Acquire));
    assert_eq!(
        seen.events(),
        ["init:capture", "start:capture", "stop:capture"]
    );
}

#[tokio::test]
async fn cancellation_and_dropped_build_future_finish_partial_init_cleanup() {
    for abort in [false, true] {
        for during_start in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let seen = Arc::new(Observed::default());
            let cancel = CancellationToken::new();
            let mut wake = Probe::new("waiting", &seen);
            wake.wait_init = !during_start;
            wake.wait_start = during_start;
            let builder = EmbeddedBuilder::from_config(config(&dir))
                .await
                .unwrap()
                .with_cancel_token(cancel.clone())
                .with_wakeword(wake);
            let task = tokio::spawn(builder.build());
            if during_start {
                seen.start_entered.notified().await;
            } else {
                seen.init_entered.notified().await;
            }
            if abort {
                task.abort();
                assert!(task.await.err().unwrap().is_cancelled());
            } else {
                cancel.cancel();
                assert!(task.await.unwrap().is_err());
            }
            tokio::time::timeout(Duration::from_secs(1), seen.stopped.notified())
                .await
                .unwrap();
            assert!(!seen.worker_running.load(Ordering::Acquire));
            if during_start {
                assert_eq!(
                    seen.events(),
                    ["init:waiting", "start:waiting", "stop:waiting"]
                );
            } else {
                assert_eq!(seen.events(), ["init:waiting", "stop:waiting"]);
            }
        }
    }
}

#[tokio::test]
async fn occupied_listener_fails_before_resources_or_modules_are_initialized() {
    let dir = tempfile::tempdir().unwrap();
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let seen = Arc::new(Observed::default());
    let mut cfg = config(&dir);
    cfg.device_runtime.listen_addr = occupied.local_addr().unwrap();
    let error = EmbeddedBuilder::from_config(cfg)
        .await
        .unwrap()
        .with_wakeword(Probe::new("wake", &seen))
        .build()
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("binding embedded gRPC"));
    assert!(seen.events().is_empty());
    assert!(!dir.path().join("run/embedded-state.sqlite3").exists());
}

#[tokio::test]
async fn independent_instances_use_config_relative_state_and_actual_discovery_ports() {
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let first_seen = Arc::new(Observed::default());
    let second_seen = Arc::new(Observed::default());
    let first = EmbeddedBuilder::from_config(config(&first_dir))
        .await
        .unwrap()
        .with_discovery(Probe::new("discovery1", &first_seen))
        .build()
        .await
        .unwrap();
    let second = EmbeddedBuilder::from_config(config(&second_dir))
        .await
        .unwrap()
        .with_discovery(Probe::new("discovery2", &second_seen))
        .build()
        .await
        .unwrap();
    assert_ne!(first.local_addr(), second.local_addr());
    assert_eq!(first.config.device_runtime.listen_addr, first.local_addr());
    assert_eq!(first.requested_config.device_runtime.listen_addr.port(), 0);
    assert_eq!(
        first
            .settings_manager
            .snapshot(false)
            .await
            .settings
            .device_runtime
            .listen_addr
            .port(),
        0
    );
    assert_eq!(
        first_seen.port.load(Ordering::Acquire),
        first.local_addr().port()
    );
    assert_eq!(
        second_seen.port.load(Ordering::Acquire),
        second.local_addr().port()
    );
    assert_eq!(
        first.config.device_runtime.state_path,
        first_dir.path().join("run/embedded-state.sqlite3")
    );
    assert_eq!(
        second.config.device_runtime.plugins_dir,
        second_dir.path().join("plugins")
    );
    assert!(first.config.device_runtime.state_path.exists());
    assert!(second.config.device_runtime.state_path.exists());
    first.shutdown().await.unwrap();
    let second_shutdown = second.shutdown_handle();
    drop(second);
    tokio::time::timeout(Duration::from_secs(1), second_shutdown.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!second_seen.worker_running.load(Ordering::Acquire));
}

#[tokio::test]
async fn blocking_native_stop_has_bounded_return_and_retains_waitable_completion() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Observed::default());
    let gate = Arc::new(Gate::default());
    let _release = ReleaseGate(gate.clone());
    let mut wake = Probe::new("blocked-native-worker", &seen);
    wake.stop_gate = Some(gate.clone());
    let mut cfg = config(&dir);
    cfg.device_runtime.shutdown_grace_ms = 30;
    cfg.device_runtime.grpc_drain_timeout_ms = 10;
    let node = EmbeddedBuilder::from_config(cfg)
        .await
        .unwrap()
        .with_wakeword(wake)
        .build()
        .await
        .unwrap();
    let shutdown = node.shutdown_handle();
    let start = tokio::time::Instant::now();
    let error = node.shutdown().await.unwrap_err();
    assert!(start.elapsed() < Duration::from_millis(500));
    assert!(error.downcast_ref::<ShutdownIncomplete>().is_some());
    assert!(
        shutdown
            .pending_modules()
            .contains(&"blocked-native-worker")
    );
    assert!(seen.worker_running.load(Ordering::Acquire));
    gate.release();
    tokio::time::timeout(Duration::from_secs(1), shutdown.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!seen.worker_running.load(Ordering::Acquire));
    assert!(shutdown.pending_modules().is_empty());
}

#[tokio::test]
async fn legacy_upload_without_eof_is_closed_and_workers_join_on_cancel_or_run_drop() {
    use alia_embedded::module::context::ModuleEvent;
    use alia_proto::audio::audio_stream_client::AudioStreamClient;
    use alia_proto::audio::{AudioChunk, AudioEncoding, PlayAudioRequest};
    use tokio_stream::wrappers::ReceiverStream;
    for abort in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(Observed::default());
        let mut cfg = config(&dir);
        cfg.dev.skip_binding = true;
        let node = EmbeddedBuilder::from_config(cfg)
            .await
            .unwrap()
            .with_audio_playback(Probe::new("playback", &seen))
            .build()
            .await
            .unwrap();
        let endpoint = format!("http://{}", node.local_addr());
        let stop = node.cancel_token.clone();
        let shutdown = node.shutdown_handle();
        let mut events = node.event_bus.subscribe();
        let server = tokio::spawn(node.run());
        let mut client = AudioStreamClient::connect(endpoint).await.unwrap();
        let mut start = tonic::Request::new(PlayAudioRequest {
            request_id: "held-open".into(),
            source: "tts".into(),
            encoding: AudioEncoding::PcmS16le as i32,
            sample_rate: 16000,
            interrupt_current: false,
        });
        start
            .metadata_mut()
            .insert("authorization", "Bearer test-dev-binding".parse().unwrap());
        client.play_audio(start).await.unwrap();
        let (_sender, receiver) = tokio::sync::mpsc::channel::<AudioChunk>(1);
        let mut upload = tonic::Request::new(ReceiverStream::new(receiver));
        upload
            .metadata_mut()
            .insert("authorization", "Bearer test-dev-binding".parse().unwrap());
        let upload = tokio::spawn(async move { client.upload_audio(upload).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(
                    events.recv().await.unwrap(),
                    ModuleEvent::PlaybackStarted { .. }
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        if abort {
            server.abort();
            assert!(server.await.err().unwrap().is_cancelled());
        } else {
            stop.cancel();
            let result = tokio::time::timeout(Duration::from_secs(1), server)
                .await
                .unwrap()
                .unwrap();
            assert!(format!("{:#}", result.unwrap_err()).contains("gRPC drain deadline"));
        }
        tokio::time::timeout(Duration::from_secs(1), shutdown.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(shutdown.pending_modules().is_empty());
        assert!(!seen.worker_running.load(Ordering::Acquire));
        assert!(
            tokio::time::timeout(Duration::from_secs(1), upload)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
    }
}

#[cfg(not(feature = "sherpa-onnx"))]
#[tokio::test]
async fn selected_feature_missing_fails_instead_of_becoming_noop() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir);
    cfg.modules.wakeword = "sherpa_onnx".into();
    let error = EmbeddedBuilder::from_config(cfg)
        .await
        .unwrap()
        .build()
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("requires the alia-embedded 'sherpa-onnx' feature"));
}

#[tokio::test]
async fn unimplemented_modules_are_errors_but_explicit_disable_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&dir);
    cfg.modules.mixer = "priority".into();
    let error = EmbeddedBuilder::from_config(cfg)
        .await
        .unwrap()
        .build()
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("priority mixer is not implemented"));
    let mut cfg = config(&dir);
    cfg.modules.face_tracker = "yunet".into();
    cfg.vision.enabled = false;
    EmbeddedBuilder::from_config(cfg)
        .await
        .unwrap()
        .build()
        .await
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn microphone_start_and_service_setup_errors_are_not_degraded_success() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Observed::default());
    let mut capture = Probe::new("capture", &seen);
    capture.fail_capture = true;
    let error = EmbeddedBuilder::from_config(config(&dir))
        .await
        .unwrap()
        .with_audio_capture(capture)
        .build()
        .await
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("starting microphone capture"));
    assert!(!seen.worker_running.load(Ordering::Acquire));
    let node = EmbeddedBuilder::from_config(config(&dir))
        .await
        .unwrap()
        .with_audio_playback(Probe::new("playback", &seen))
        .build()
        .await
        .unwrap();
    let handle = node.shutdown_handle();
    std::fs::remove_file(&node.config.identity.keypair_path).unwrap();
    let error = node.run().await.unwrap_err();
    assert!(format!("{error:#}").contains("loading device public key"));
    handle.wait().await.unwrap();
    assert!(!seen.worker_running.load(Ordering::Acquire));
}

#[tokio::test]
async fn cancellation_during_discovery_publication_rolls_back_started_modules() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Observed::default());
    let mut discovery = Probe::new("discovery", &seen);
    discovery.wait_metadata = true;
    let cancel = CancellationToken::new();
    let builder = EmbeddedBuilder::from_config(config(&dir))
        .await
        .unwrap()
        .with_cancel_token(cancel.clone())
        .with_discovery(discovery);
    let build = tokio::spawn(builder.build());
    seen.metadata_entered.notified().await;
    cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), build)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert!(!seen.worker_running.load(Ordering::Acquire));
    assert_eq!(
        seen.events(),
        ["init:discovery", "start:discovery", "stop:discovery"]
    );
}
