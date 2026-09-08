use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::VisionDebugConfig;

use super::context::ModuleContext;
use super::lifecycle::{ModuleHealth, ModuleLifecycle};
use super::traits::FaceDetection;

const MJPEG_BOUNDARY: &str = "alia-vision-frame";

#[derive(Debug, Clone, Serialize)]
pub struct VisionDebugFace {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub confidence: f32,
}

impl From<&FaceDetection> for VisionDebugFace {
    fn from(face: &FaceDetection) -> Self {
        Self {
            x: face.x,
            y: face.y,
            width: face.width,
            height: face.height,
            confidence: face.confidence,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VisionDebugFrameMeta {
    pub sequence: u64,
    pub timestamp_us: u64,
    pub width: i32,
    pub height: i32,
    pub detector_width: i32,
    pub detector_height: i32,
    pub inference_ms: f32,
    pub encode_ms: f32,
    pub faces: Vec<VisionDebugFace>,
    pub primary_face: Option<VisionDebugFace>,
}

#[derive(Debug, Clone)]
pub struct VisionDebugFrame {
    pub meta: VisionDebugFrameMeta,
    pub jpeg: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct VisionDebugStatus {
    pub enabled: bool,
    pub stream_fps: u32,
    pub draw_overlay: bool,
    pub latest: Option<VisionDebugFrameMeta>,
    pub latest_age_ms: Option<u64>,
}

pub struct VisionDebugHub {
    config: VisionDebugConfig,
    latest: StdMutex<Option<Arc<VisionDebugFrame>>>,
    sequence: AtomicU64,
    updates: watch::Sender<u64>,
}

impl VisionDebugHub {
    pub fn new(config: VisionDebugConfig) -> Self {
        let (updates, _) = watch::channel(0);
        Self {
            config,
            latest: StdMutex::new(None),
            sequence: AtomicU64::new(0),
            updates,
        }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn draw_overlay(&self) -> bool {
        self.config.draw_overlay
    }

    pub fn jpeg_quality(&self) -> i32 {
        self.config.jpeg_quality.clamp(1, 100)
    }

    pub fn frame_period(&self) -> Duration {
        let fps = self.config.stream_fps.clamp(1, 60);
        Duration::from_millis((1_000 / u64::from(fps)).max(1))
    }

    pub fn publish(&self, mut frame: VisionDebugFrame) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        frame.meta.sequence = sequence;
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(Arc::new(frame));
        }
        let _ = self.updates.send(sequence);
    }

    pub fn latest(&self) -> Option<Arc<VisionDebugFrame>> {
        self.latest
            .lock()
            .ok()
            .and_then(|frame| frame.as_ref().cloned())
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.updates.subscribe()
    }

    fn status(&self) -> VisionDebugStatus {
        let latest = self.latest();
        let latest_age_ms = latest
            .as_ref()
            .map(|frame| timestamp_us().saturating_sub(frame.meta.timestamp_us) / 1_000);
        VisionDebugStatus {
            enabled: self.config.enabled,
            stream_fps: self.config.stream_fps,
            draw_overlay: self.config.draw_overlay,
            latest: latest.as_ref().map(|frame| frame.meta.clone()),
            latest_age_ms,
        }
    }
}

pub fn vision_debug_meta(
    width: i32,
    height: i32,
    detector_width: i32,
    detector_height: i32,
    inference_ms: f32,
    encode_ms: f32,
    faces: &[FaceDetection],
    primary_face: Option<&FaceDetection>,
) -> VisionDebugFrameMeta {
    VisionDebugFrameMeta {
        sequence: 0,
        timestamp_us: timestamp_us(),
        width,
        height,
        detector_width,
        detector_height,
        inference_ms,
        encode_ms,
        faces: faces.iter().map(VisionDebugFace::from).collect(),
        primary_face: primary_face.map(VisionDebugFace::from),
    }
}

pub struct VisionDebugServer {
    config: VisionDebugConfig,
    hub: Arc<VisionDebugHub>,
    running: Arc<AtomicBool>,
    cancel: Mutex<Option<CancellationToken>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl VisionDebugServer {
    pub fn new(config: VisionDebugConfig, hub: Arc<VisionDebugHub>) -> Self {
        Self {
            config,
            hub,
            running: Arc::new(AtomicBool::new(false)),
            cancel: Mutex::new(None),
            task: Mutex::new(None),
        }
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for VisionDebugServer {
    fn name(&self) -> &'static str {
        "vision_debug_web"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> Result<()> {
        if self.config.enabled {
            parse_bind_addr(&self.config.bind_addr)?;
        }
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        let addr = parse_bind_addr(&self.config.bind_addr)?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding vision debug web server to {addr}"))?;
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let cancel = CancellationToken::new();
        let shutdown = cancel.clone();
        let hub = self.hub.clone();
        let app = Router::new()
            .route("/debug/vision", get(vision_page))
            .route("/debug/vision/status", get(vision_status))
            .route("/debug/vision/snapshot.jpg", get(vision_snapshot))
            .route("/debug/vision.mjpg", get(vision_mjpeg))
            .with_state(hub);
        let running = self.running.clone();

        tracing::info!(
            %addr,
            "vision debug web server listening at /debug/vision"
        );

        *self.cancel.lock().await = Some(cancel);
        *self.task.lock().await = Some(tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown.cancelled().await;
                })
                .await;
            if let Err(error) = result {
                tracing::warn!(error = %error, "vision debug web server stopped with error");
            }
            running.store(false, Ordering::SeqCst);
        }));
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if let Some(cancel) = self.cancel.lock().await.take() {
            cancel.cancel();
        }
        if let Some(task) = self.task.lock().await.take() {
            task.await.context("joining vision worker")?;
        }
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if !self.config.enabled {
            ModuleHealth::Degraded
        } else if self.running.load(Ordering::SeqCst) {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Unhealthy
        }
    }
}

fn parse_bind_addr(value: &str) -> Result<SocketAddr> {
    value
        .parse()
        .with_context(|| format!("invalid vision_debug.bind_addr '{value}'"))
}

async fn vision_page() -> Html<&'static str> {
    Html(VISION_DEBUG_HTML)
}

async fn vision_status(State(hub): State<Arc<VisionDebugHub>>) -> Json<VisionDebugStatus> {
    Json(hub.status())
}

async fn vision_snapshot(State(hub): State<Arc<VisionDebugHub>>) -> Response {
    let Some(frame) = hub.latest() else {
        return (StatusCode::NO_CONTENT, "no vision frame available yet").into_response();
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-frame-sequence", frame.meta.sequence.to_string())
        .body(Body::from(frame.jpeg.clone()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn vision_mjpeg(State(hub): State<Arc<VisionDebugHub>>) -> Response {
    let stream = async_stream::stream! {
        let mut rx = hub.subscribe();
        let mut last_sequence = 0_u64;

        if let Some(frame) = hub.latest() {
            last_sequence = frame.meta.sequence;
            yield Ok::<Vec<u8>, Infallible>(mjpeg_part(&frame));
        }

        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let sequence = *rx.borrow_and_update();
            if sequence == last_sequence {
                continue;
            }
            if let Some(frame) = hub.latest() {
                last_sequence = frame.meta.sequence;
                yield Ok::<Vec<u8>, Infallible>(mjpeg_part(&frame));
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/x-mixed-replace; boundary={MJPEG_BOUNDARY}"),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn mjpeg_part(frame: &VisionDebugFrame) -> Vec<u8> {
    let mut part = format!(
        "--{MJPEG_BOUNDARY}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nX-Frame-Sequence: {}\r\n\r\n",
        frame.jpeg.len(),
        frame.meta.sequence
    )
    .into_bytes();
    part.extend_from_slice(&frame.jpeg);
    part.extend_from_slice(b"\r\n");
    part
}

fn timestamp_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros() as u64)
        .unwrap_or(0)
}

const VISION_DEBUG_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Alia Vision Debug</title>
  <style>
    :root {
      color-scheme: dark;
      --bg: #11130f;
      --panel: #191c17;
      --ink: #f2f0e7;
      --muted: #9ba08f;
      --line: #33392d;
      --accent: #d5ff5f;
      --hot: #ff6f4a;
      --ok: #7dffa3;
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      background:
        linear-gradient(90deg, rgba(255,255,255,.035) 1px, transparent 1px) 0 0 / 48px 48px,
        linear-gradient(0deg, rgba(255,255,255,.026) 1px, transparent 1px) 0 0 / 48px 48px,
        var(--bg);
      color: var(--ink);
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", monospace;
    }
    main {
      width: min(1280px, calc(100vw - 32px));
      margin: 0 auto;
      padding: 24px 0;
    }
    header {
      display: grid;
      grid-template-columns: 1fr auto;
      gap: 16px;
      align-items: end;
      border-bottom: 1px solid var(--line);
      padding-bottom: 16px;
      margin-bottom: 18px;
    }
    h1 {
      margin: 0;
      font-size: clamp(28px, 5vw, 64px);
      line-height: .9;
      letter-spacing: 0;
      text-transform: uppercase;
    }
    .status {
      display: grid;
      grid-template-columns: repeat(4, minmax(120px, 1fr));
      gap: 10px;
      margin-bottom: 18px;
    }
    .metric {
      border: 1px solid var(--line);
      background: color-mix(in srgb, var(--panel), transparent 10%);
      padding: 12px;
      min-height: 76px;
    }
    .label {
      color: var(--muted);
      font-size: 11px;
      text-transform: uppercase;
    }
    .value {
      margin-top: 8px;
      color: var(--accent);
      font-size: 22px;
      overflow-wrap: anywhere;
    }
    .stage {
      border: 1px solid var(--line);
      background: #050604;
      min-height: 320px;
      display: grid;
      place-items: center;
      position: relative;
      overflow: hidden;
    }
    .stage::before {
      content: "";
      position: absolute;
      inset: 0;
      pointer-events: none;
      background: radial-gradient(circle at 50% 0%, rgba(213,255,95,.14), transparent 38%);
      mix-blend-mode: screen;
    }
    img {
      display: block;
      width: 100%;
      height: auto;
      max-height: calc(100vh - 230px);
      object-fit: contain;
      position: relative;
      z-index: 1;
    }
    .waiting {
      padding: 32px;
      color: var(--muted);
      text-transform: uppercase;
      letter-spacing: 0;
    }
    footer {
      display: flex;
      justify-content: space-between;
      gap: 12px;
      color: var(--muted);
      font-size: 12px;
      border-top: 1px solid var(--line);
      margin-top: 18px;
      padding-top: 12px;
    }
    .ok { color: var(--ok); }
    .hot { color: var(--hot); }
    @media (max-width: 760px) {
      header { grid-template-columns: 1fr; }
      .status { grid-template-columns: 1fr 1fr; }
      main { width: min(100vw - 20px, 1280px); padding: 14px 0; }
    }
  </style>
</head>
<body>
  <main>
    <header>
      <h1>Alia Vision Debug</h1>
      <div class="label" id="clock">waiting</div>
    </header>
    <section class="status">
      <div class="metric"><div class="label">Stream</div><div class="value" id="stream">offline</div></div>
      <div class="metric"><div class="label">Faces</div><div class="value" id="faces">0</div></div>
      <div class="metric"><div class="label">Inference</div><div class="value" id="inference">--</div></div>
      <div class="metric"><div class="label">Frame</div><div class="value" id="frame">--</div></div>
    </section>
    <section class="stage">
      <img id="preview" src="/debug/vision.mjpg" alt="">
      <div class="waiting" id="waiting">waiting for camera frames</div>
    </section>
    <footer>
      <span id="primary">primary face: none</span>
      <span>/debug/vision/snapshot.jpg</span>
    </footer>
  </main>
  <script>
    const el = (id) => document.getElementById(id);
    const preview = el('preview');
    const waiting = el('waiting');

    preview.addEventListener('load', () => {
      waiting.style.display = 'none';
      preview.style.display = 'block';
    });
    preview.addEventListener('error', () => {
      preview.style.display = 'none';
      waiting.style.display = 'block';
    });

    async function refreshStatus() {
      try {
        const response = await fetch('/debug/vision/status', { cache: 'no-store' });
        const data = await response.json();
        const latest = data.latest;
        el('clock').textContent = new Date().toLocaleTimeString();
        el('stream').textContent = latest ? `${data.stream_fps} fps` : 'waiting';
        el('stream').className = latest ? 'value ok' : 'value hot';
        el('faces').textContent = latest ? latest.faces.length : '0';
        el('inference').textContent = latest ? `${latest.inference_ms.toFixed(1)} ms` : '--';
        el('frame').textContent = latest ? `${latest.width}x${latest.height}` : '--';
        if (latest && latest.primary_face) {
          const face = latest.primary_face;
          el('primary').textContent =
            `primary face: ${face.confidence.toFixed(2)} @ ${Math.round(face.x)},${Math.round(face.y)} ${Math.round(face.width)}x${Math.round(face.height)}`;
        } else {
          el('primary').textContent = 'primary face: none';
        }
      } catch (_) {
        el('stream').textContent = 'offline';
        el('stream').className = 'value hot';
      }
    }

    refreshStatus();
    setInterval(refreshStatus, 500);
  </script>
</body>
</html>"#;
