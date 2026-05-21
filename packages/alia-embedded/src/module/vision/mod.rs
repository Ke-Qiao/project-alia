use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::config::VisionConfig;
use crate::module::context::ModuleContext;
use crate::module::lifecycle::{ModuleHealth, ModuleLifecycle};
use crate::module::motion::MotionRuntime;
use crate::module::traits::{FaceDetection, FaceTracker};
use crate::module::vision_debug::VisionDebugHub;
#[cfg(feature = "vision-opencv")]
use crate::module::vision_debug::{VisionDebugFrame, vision_debug_meta};

pub struct YuNetFaceTracker {
    config: VisionConfig,
    motion: Arc<MotionRuntime>,
    debug_hub: Option<Arc<VisionDebugHub>>,
    latest_faces: Arc<StdMutex<Vec<FaceDetection>>>,
    running: Arc<AtomicBool>,
    cancel: Mutex<Option<CancellationToken>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl YuNetFaceTracker {
    pub fn new(
        config: VisionConfig,
        motion: Arc<MotionRuntime>,
        debug_hub: Option<Arc<VisionDebugHub>>,
    ) -> Result<Self> {
        if config.enabled {
            validate_vision_config(&config)?;
        }
        Ok(Self {
            config,
            motion,
            debug_hub,
            latest_faces: Arc::new(StdMutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            cancel: Mutex::new(None),
            task: Mutex::new(None),
        })
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for YuNetFaceTracker {
    fn name(&self) -> &'static str {
        "yunet_face_tracker"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> Result<()> {
        if self.config.enabled {
            validate_vision_config(&self.config)?;
        }
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let cancel = CancellationToken::new();
        let latest_faces = self.latest_faces.clone();
        let running = self.running.clone();
        let config = self.config.clone();
        let motion = self.motion.clone();
        let debug_hub = self.debug_hub.clone();
        let runtime = tokio::runtime::Handle::current();

        *self.cancel.lock().await = Some(cancel.clone());
        *self.task.lock().await = Some(tokio::task::spawn_blocking(move || {
            let result = run_yunet_loop(config, latest_faces, motion, debug_hub, runtime, cancel);
            if let Err(error) = result {
                warn!(error = %format!("{error:#}"), "YuNet face tracker stopped with error");
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
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
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

#[tonic::async_trait]
impl FaceTracker for YuNetFaceTracker {
    async fn get_faces(&self) -> Result<Vec<FaceDetection>> {
        Ok(self
            .latest_faces
            .lock()
            .map(|faces| faces.clone())
            .unwrap_or_default())
    }

    fn is_tracking(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

fn validate_vision_config(config: &VisionConfig) -> Result<()> {
    if config.camera_width <= 0 || config.camera_height <= 0 {
        bail!("vision camera dimensions must be positive");
    }
    if config.input_width <= 0 || config.input_height <= 0 {
        bail!("vision detector input dimensions must be positive");
    }
    if config.detector_fps == 0 {
        bail!("vision detector_fps must be greater than zero");
    }
    Ok(())
}

fn should_emit_face_update(
    previous: Option<(f32, f32)>,
    next: (f32, f32),
    min_delta_px: f32,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    let dx = next.0 - previous.0;
    let dy = next.1 - previous.1;
    (dx * dx + dy * dy).sqrt() >= min_delta_px
}

#[cfg(feature = "vision-opencv")]
fn run_yunet_loop(
    config: VisionConfig,
    latest_faces: Arc<StdMutex<Vec<FaceDetection>>>,
    motion: Arc<MotionRuntime>,
    debug_hub: Option<Arc<VisionDebugHub>>,
    runtime: tokio::runtime::Handle,
    cancel: CancellationToken,
) -> Result<()> {
    use std::time::Instant;

    use anyhow::Context;
    use opencv::core::Size;
    use opencv::objdetect::FaceDetectorYN;
    use opencv::prelude::*;
    use opencv::{imgproc, videoio};

    if !config.yunet_model_path.exists() {
        bail!(
            "YuNet model file does not exist: {}",
            config.yunet_model_path.display()
        );
    }
    validate_yunet_model_payload(&config.yunet_model_path)?;
    validate_opencv_yunet_compatibility(&config.yunet_model_path)?;

    let detector_size = Size::new(config.input_width, config.input_height);
    let mut detector = FaceDetectorYN::create(
        config.yunet_model_path.to_string_lossy().as_ref(),
        "",
        detector_size,
        config.score_threshold,
        config.nms_threshold,
        config.top_k,
        0,
        0,
    )
    .with_context(|| {
        format!(
            "creating OpenCV YuNet detector from {} with input {}x{}",
            config.yunet_model_path.display(),
            config.input_width,
            config.input_height
        )
    })?;
    detector
        .set_input_size(detector_size)
        .context("setting YuNet input size")?;

    let mut capture = videoio::VideoCapture::new(config.camera_index, videoio::CAP_ANY)
        .with_context(|| format!("opening camera capture index {}", config.camera_index))?;
    capture
        .set(videoio::CAP_PROP_FRAME_WIDTH, config.camera_width as f64)
        .with_context(|| format!("setting camera width to {}", config.camera_width))?;
    capture
        .set(videoio::CAP_PROP_FRAME_HEIGHT, config.camera_height as f64)
        .with_context(|| format!("setting camera height to {}", config.camera_height))?;
    capture
        .set(videoio::CAP_PROP_FPS, config.capture_fps)
        .with_context(|| format!("setting camera FPS to {}", config.capture_fps))?;
    if !capture.is_opened()? {
        bail!("camera {} is not open", config.camera_index);
    }

    let target_period = Duration::from_millis((1000 / config.detector_fps.max(1) as u64).max(1));
    let min_motion_interval = Duration::from_millis(config.min_motion_interval_ms);
    let debug_period = debug_hub
        .as_ref()
        .filter(|hub| hub.enabled())
        .map(|hub| hub.frame_period());
    let mut previous_center: Option<(f32, f32)> = None;
    let mut last_motion = Instant::now() - min_motion_interval;
    let mut last_debug_frame =
        Instant::now() - debug_period.unwrap_or_else(|| Duration::from_secs(60));

    while !cancel.is_cancelled() {
        let loop_started = Instant::now();
        let mut frame = Mat::default();
        if !capture.read(&mut frame)? || frame.empty() {
            std::thread::sleep(target_period);
            continue;
        }

        let frame_w = frame.cols().max(1) as f32;
        let frame_h = frame.rows().max(1) as f32;
        let mut detector_frame = Mat::default();
        imgproc::resize(
            &frame,
            &mut detector_frame,
            detector_size,
            0.0,
            0.0,
            imgproc::INTER_LINEAR,
        )
        .context("resizing frame for YuNet")?;

        let mut raw_faces = Mat::default();
        let inference_started = Instant::now();
        detector
            .detect(&detector_frame, &mut raw_faces)
            .context("running YuNet face detector")?;
        let inference_ms = inference_started.elapsed().as_secs_f32() * 1_000.0;
        let faces = parse_yunet_faces(
            &raw_faces,
            frame_w / config.input_width as f32,
            frame_h / config.input_height as f32,
        )?;
        if let Ok(mut latest) = latest_faces.lock() {
            *latest = faces.clone();
        }

        let primary = primary_face(&faces);

        if let (Some(hub), Some(period)) = (debug_hub.as_ref(), debug_period)
            && last_debug_frame.elapsed() >= period
        {
            let encode_started = Instant::now();
            match encode_debug_frame(&frame, &faces, primary, hub) {
                Ok(jpeg) => {
                    let encode_ms = encode_started.elapsed().as_secs_f32() * 1_000.0;
                    hub.publish(VisionDebugFrame {
                        meta: vision_debug_meta(
                            frame.cols(),
                            frame.rows(),
                            config.input_width,
                            config.input_height,
                            inference_ms,
                            encode_ms,
                            &faces,
                            primary,
                        ),
                        jpeg,
                    });
                    last_debug_frame = Instant::now();
                }
                Err(error) => warn!(error = %error, "vision debug frame encode failed"),
            }
        }

        if let Some(face) = primary {
            let center = (face.x + face.width * 0.5, face.y + face.height * 0.5);
            let elapsed = last_motion.elapsed();
            if elapsed >= min_motion_interval
                && should_emit_face_update(previous_center, center, config.min_face_delta_px)
            {
                match runtime
                    .block_on(motion.track_face_center(center.0, center.1, frame_w, frame_h))
                {
                    Ok(true) => {
                        previous_center = Some(center);
                        last_motion = Instant::now();
                    }
                    Ok(false) => tracing::debug!("motion runtime skipped face tracking update"),
                    Err(error) => warn!(error = %error, "face tracking motion update failed"),
                }
            }
        }

        let elapsed = loop_started.elapsed();
        if elapsed < target_period {
            std::thread::sleep(target_period - elapsed);
        }
    }

    Ok(())
}

#[cfg(feature = "vision-opencv")]
fn validate_opencv_yunet_compatibility(path: &std::path::Path) -> Result<()> {
    let major = opencv::core::get_version_major();
    let minor = opencv::core::get_version_minor();
    let revision = opencv::core::get_version_revision();
    let model_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if model_name.contains("2023mar") && (major < 4 || (major == 4 && minor < 8)) {
        bail!(
            "OpenCV {major}.{minor}.{revision} is too old for {model_name}; OpenCV 4.6/4.7 can parse the model but fails during FaceDetectorYN::detect with 'Layer with requested id=-1 not found'. Install OpenCV 4.8 or newer, or point vision.yunet_model_path at an older YuNet model compatible with your OpenCV build."
        );
    }
    Ok(())
}

#[cfg(feature = "vision-opencv")]
fn validate_yunet_model_payload(path: &std::path::Path) -> Result<()> {
    use anyhow::Context;
    use std::io::Read;

    let metadata = std::fs::metadata(path)
        .with_context(|| format!("reading metadata for {}", path.display()))?;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut header = [0_u8; 128];
    let n = file
        .read(&mut header)
        .with_context(|| format!("reading header from {}", path.display()))?;
    let head = String::from_utf8_lossy(&header[..n]);
    if head.starts_with("version https://git-lfs.github.com/spec/v1") {
        bail!(
            "YuNet model file is a Git LFS pointer, not an ONNX payload: {}. Re-download it from https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
            path.display()
        );
    }
    let trimmed = head.trim_start();
    if trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<html") {
        bail!(
            "YuNet model file contains HTML, not an ONNX payload: {}",
            path.display()
        );
    }
    if metadata.len() < 100_000 {
        bail!(
            "YuNet model file is unexpectedly small ({} bytes): {}",
            metadata.len(),
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(feature = "vision-opencv"))]
fn run_yunet_loop(
    _config: VisionConfig,
    _latest_faces: Arc<StdMutex<Vec<FaceDetection>>>,
    _motion: Arc<MotionRuntime>,
    _debug_hub: Option<Arc<VisionDebugHub>>,
    _runtime: tokio::runtime::Handle,
    _cancel: CancellationToken,
) -> Result<()> {
    bail!("YuNet face tracker requires the alia-embedded 'vision-opencv' feature")
}

#[cfg(feature = "vision-opencv")]
fn parse_yunet_faces(
    faces: &opencv::core::Mat,
    scale_x: f32,
    scale_y: f32,
) -> Result<Vec<FaceDetection>> {
    use opencv::prelude::*;

    let mut detections = Vec::new();
    for row in 0..faces.rows() {
        detections.push(FaceDetection {
            x: *faces.at_2d::<f32>(row, 0)? * scale_x,
            y: *faces.at_2d::<f32>(row, 1)? * scale_y,
            width: *faces.at_2d::<f32>(row, 2)? * scale_x,
            height: *faces.at_2d::<f32>(row, 3)? * scale_y,
            confidence: *faces.at_2d::<f32>(row, 14)?,
        });
    }
    Ok(detections)
}

#[cfg(feature = "vision-opencv")]
fn encode_debug_frame(
    frame: &opencv::core::Mat,
    faces: &[FaceDetection],
    primary: Option<&FaceDetection>,
    hub: &VisionDebugHub,
) -> Result<Vec<u8>> {
    use anyhow::Context;
    use opencv::core::{Point, Scalar, Vector};
    use opencv::prelude::*;
    use opencv::{imgcodecs, imgproc};

    let mut debug_frame = frame.try_clone().context("cloning vision debug frame")?;
    if hub.draw_overlay() {
        let width = debug_frame.cols().max(1);
        let height = debug_frame.rows().max(1);
        let center_color = Scalar::new(95.0, 255.0, 213.0, 0.0);
        let face_color = Scalar::new(74.0, 111.0, 255.0, 0.0);
        let primary_color = Scalar::new(95.0, 255.0, 125.0, 0.0);
        let text_color = Scalar::new(231.0, 240.0, 242.0, 0.0);

        imgproc::line(
            &mut debug_frame,
            Point::new(width / 2, 0),
            Point::new(width / 2, height),
            center_color,
            1,
            imgproc::LINE_8,
            0,
        )
        .context("drawing vision debug vertical center")?;
        imgproc::line(
            &mut debug_frame,
            Point::new(0, height / 2),
            Point::new(width, height / 2),
            center_color,
            1,
            imgproc::LINE_8,
            0,
        )
        .context("drawing vision debug horizontal center")?;

        for face in faces {
            let is_primary = primary.is_some_and(|candidate| std::ptr::eq(candidate, face));
            let rect = face_rect(face, width, height);
            let color = if is_primary {
                primary_color
            } else {
                face_color
            };
            imgproc::rectangle(&mut debug_frame, rect, color, 2, imgproc::LINE_8, 0)
                .context("drawing vision debug face box")?;
            let face_center = Point::new(rect.x + rect.width / 2, rect.y + rect.height / 2);
            imgproc::circle(
                &mut debug_frame,
                face_center,
                4,
                color,
                -1,
                imgproc::LINE_8,
                0,
            )
            .context("drawing vision debug face center")?;
            imgproc::put_text(
                &mut debug_frame,
                &format!("{:.2}", face.confidence),
                Point::new(rect.x, (rect.y - 6).max(14)),
                imgproc::FONT_HERSHEY_SIMPLEX,
                0.5,
                text_color,
                1,
                imgproc::LINE_AA,
                false,
            )
            .context("drawing vision debug confidence")?;
        }

        imgproc::put_text(
            &mut debug_frame,
            &format!("faces: {}", faces.len()),
            Point::new(12, 24),
            imgproc::FONT_HERSHEY_SIMPLEX,
            0.6,
            text_color,
            1,
            imgproc::LINE_AA,
            false,
        )
        .context("drawing vision debug label")?;
    }

    let mut encoded = Vector::<u8>::new();
    let mut params = Vector::<i32>::new();
    params.push(imgcodecs::IMWRITE_JPEG_QUALITY);
    params.push(hub.jpeg_quality());
    imgcodecs::imencode(".jpg", &debug_frame, &mut encoded, &params)
        .context("encoding vision debug JPEG")?;
    Ok(encoded.to_vec())
}

#[cfg(feature = "vision-opencv")]
fn face_rect(face: &FaceDetection, frame_width: i32, frame_height: i32) -> opencv::core::Rect {
    let max_x = frame_width.saturating_sub(1) as f32;
    let max_y = frame_height.saturating_sub(1) as f32;
    let x1 = face.x.clamp(0.0, max_x).round() as i32;
    let y1 = face.y.clamp(0.0, max_y).round() as i32;
    let x2 = (face.x + face.width).clamp(0.0, frame_width as f32).round() as i32;
    let y2 = (face.y + face.height)
        .clamp(0.0, frame_height as f32)
        .round() as i32;
    opencv::core::Rect::new(x1, y1, (x2 - x1).max(1), (y2 - y1).max(1))
}

fn primary_face(faces: &[FaceDetection]) -> Option<&FaceDetection> {
    faces.iter().max_by(|left, right| {
        let left_score = left.confidence * left.width.max(0.0) * left.height.max(0.0);
        let right_score = right.confidence * right.width.max(0.0) * right.height.max(0.0);
        left_score.total_cmp(&right_score)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_update_delta_uses_pixel_distance() {
        assert!(should_emit_face_update(None, (10.0, 10.0), 6.0));
        assert!(!should_emit_face_update(
            Some((10.0, 10.0)),
            (13.0, 14.0),
            6.0
        ));
        assert!(should_emit_face_update(
            Some((10.0, 10.0)),
            (16.0, 10.0),
            6.0
        ));
    }

    #[test]
    fn primary_face_prefers_confident_large_face() {
        let small = FaceDetection {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
            confidence: 0.95,
        };
        let large = FaceDetection {
            x: 0.0,
            y: 0.0,
            width: 80.0,
            height: 80.0,
            confidence: 0.7,
        };
        let faces = vec![small, large.clone()];
        assert_eq!(
            primary_face(&faces).map(|face| face.width),
            Some(large.width)
        );
    }
}
