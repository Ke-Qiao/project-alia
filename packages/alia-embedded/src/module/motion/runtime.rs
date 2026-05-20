use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::config::MotionConfig;
use crate::module::context::ModuleContext;
use crate::module::lifecycle::{ModuleHealth, ModuleLifecycle};

use super::neck_mapper::NeckMapper;
use super::servo::{ServoBackend, create_servo_backend};

pub struct MotionRuntime {
    config: MotionConfig,
    mapper: Option<Mutex<NeckMapper>>,
    servo: Arc<dyn ServoBackend>,
    state: Mutex<MotionState>,
}

#[derive(Default)]
struct MotionState {
    busy: bool,
    cancel: Option<CancellationToken>,
    smoothed_face_rot: [f32; 3],
}

#[derive(Debug, Clone, Deserialize)]
struct ExpressionPlan {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timeline: Vec<ExpressionTimelineAction>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExpressionTimelineAction {
    #[serde(default)]
    action: String,
    #[serde(default)]
    intensity: f32,
    #[serde(default)]
    duration_ms: u32,
    #[serde(default)]
    params: serde_json::Value,
}

impl MotionRuntime {
    pub fn new(config: MotionConfig) -> Result<Self> {
        let servo: Arc<dyn ServoBackend> = create_servo_backend(&config)?.into();
        let mapper = if config.enabled {
            Some(Mutex::new(NeckMapper::load(&config.neck_config_file)?))
        } else {
            None
        };
        Ok(Self {
            config,
            mapper,
            servo,
            state: Mutex::new(MotionState::default()),
        })
    }

    pub fn disabled(config: MotionConfig) -> Self {
        let servo: Arc<dyn ServoBackend> = create_servo_backend(&MotionConfig {
            hardware_enabled: false,
            ..config.clone()
        })
        .expect("mock servo backend should not fail")
        .into();
        Self {
            config: MotionConfig {
                enabled: false,
                ..config
            },
            mapper: None,
            servo,
            state: Mutex::new(MotionState::default()),
        }
    }

    pub async fn capabilities_json(&self) -> serde_json::Value {
        let state = self.state.lock().await;
        let ready = self.config.enabled && self.mapper.is_some();
        json!({
            "adapter": "embedded_native_motion_v1",
            "state_source": "embedded_native_motion",
            "capabilities": {
                "look_at_user": {
                    "supported": ready,
                    "params": ["intensity", "duration_ms", "target", "rotation", "face_center"],
                    "safety_class": "low_risk",
                    "max_intensity": 0.45,
                    "max_duration_ms": 1200,
                    "max_frequency_per_min": 6
                },
                "small_nod": {
                    "supported": ready,
                    "params": ["intensity", "duration_ms"],
                    "safety_class": "low_risk",
                    "max_intensity": 0.45,
                    "max_duration_ms": 900,
                    "max_frequency_per_min": 6
                },
                "small_bow": {
                    "supported": ready,
                    "params": ["intensity", "duration_ms"],
                    "safety_class": "low_risk",
                    "max_intensity": 0.35,
                    "max_duration_ms": 1200,
                    "max_frequency_per_min": 4
                },
                "play_animation": {
                    "supported": ready,
                    "params": ["name", "file", "fps", "loops", "duration_ms"],
                    "safety_class": "low_risk",
                    "max_intensity": 0.45,
                    "max_duration_ms": 5000,
                    "max_frequency_per_min": 4
                }
            },
            "current_state": {
                "busy": state.busy,
                "motion_adapter_registered": ready,
                "thermal_limit": null,
                "battery_saver": null,
                "battery_level": null
            },
            "receipt_model": {
                "statuses": ["accepted", "started", "completed", "failed", "timed_out"],
                "state_quality": "unknown_when_no_sensor_provider"
            }
        })
    }

    pub async fn execute_expression_json(&self, plan_json: &str) -> Result<serde_json::Value> {
        if !self.config.enabled || self.mapper.is_none() {
            return Ok(expression_result(
                "skipped",
                None,
                Vec::new(),
                vec![skipped("*", "embedded native motion is disabled")],
                true,
            ));
        }

        let plan: ExpressionPlan =
            serde_json::from_str(plan_json).context("parsing expression plan")?;
        if plan.timeline.is_empty() {
            return Ok(expression_result(
                "skipped",
                plan.id,
                Vec::new(),
                vec![skipped("execute_expression", "empty expression timeline")],
                true,
            ));
        }

        let cancel = CancellationToken::new();
        {
            let mut state = self.state.lock().await;
            if state.busy {
                return Ok(expression_result(
                    "blocked",
                    plan.id,
                    Vec::new(),
                    plan.timeline
                        .iter()
                        .map(|action| skipped(&action.action, "embedded native motion is busy"))
                        .collect(),
                    true,
                ));
            }
            state.busy = true;
            state.cancel = Some(cancel.clone());
        }

        let plan_id = plan.id.clone();
        let mut executed = Vec::new();
        let mut skipped_actions = Vec::new();
        for action in &plan.timeline {
            if cancel.is_cancelled() {
                skipped_actions.push(skipped(&action.action, "expression execution cancelled"));
                continue;
            }
            match self.execute_action(action, &cancel).await {
                Ok(()) => executed.push(action.action.clone()),
                Err(error) => skipped_actions.push(skipped(&action.action, error.to_string())),
            }
        }

        {
            let mut state = self.state.lock().await;
            state.busy = false;
            state.cancel = None;
        }

        let status = if skipped_actions.is_empty() {
            "completed"
        } else if executed.is_empty() {
            "skipped"
        } else {
            "partial"
        };
        Ok(expression_result(
            status,
            plan_id,
            executed,
            skipped_actions,
            true,
        ))
    }

    pub async fn cancel_expression_json(&self, reason: &str) -> serde_json::Value {
        let mut state = self.state.lock().await;
        if let Some(cancel) = state.cancel.take() {
            cancel.cancel();
        }
        state.busy = false;
        expression_result(
            "completed",
            None,
            vec!["cancel_expression".to_string()],
            Vec::new(),
            true,
        )
        .with_reason(reason)
    }

    pub async fn track_face_center(
        &self,
        cx: f32,
        cy: f32,
        frame_w: f32,
        frame_h: f32,
    ) -> Result<bool> {
        if !self.config.enabled || self.mapper.is_none() {
            return Ok(false);
        }
        if self.state.lock().await.busy {
            return Ok(false);
        }

        let rot = self.face_center_to_rotation(cx, cy, frame_w, frame_h).await;
        self.set_rotation(rot, true).await?;
        Ok(true)
    }

    async fn execute_action(
        &self,
        action: &ExpressionTimelineAction,
        cancel: &CancellationToken,
    ) -> Result<()> {
        match action.action.as_str() {
            "small_nod" => {
                let amp = scaled_amplitude(action.intensity, 3.0, 20.0, 12.0);
                let duration = action.duration_ms.clamp(240, 900);
                self.play_rotation_path(
                    &[
                        [0.0, 0.0, 0.0],
                        [amp, 0.0, 0.0],
                        [-amp * 0.55, 0.0, 0.0],
                        [0.0, 0.0, 0.0],
                    ],
                    duration,
                    false,
                    cancel,
                )
                .await
            }
            "small_bow" => {
                let amp = scaled_amplitude(action.intensity, 4.0, 22.0, 12.0);
                let duration = action.duration_ms.clamp(300, 1200);
                self.play_rotation_path(
                    &[
                        [0.0, 0.0, 0.0],
                        [amp, 0.0, 0.0],
                        [amp * 0.85, 0.0, 0.0],
                        [0.0, 0.0, 0.0],
                    ],
                    duration,
                    false,
                    cancel,
                )
                .await
            }
            "look_at_user" => {
                let rot = self.rotation_from_params(&action.params).await?;
                let duration = action.duration_ms.clamp(120, 1200);
                self.play_rotation_path(&[[0.0, 0.0, 0.0], rot], duration, true, cancel)
                    .await
            }
            "play_animation" | "play_anim" => self.play_animation(action, cancel).await,
            other => bail!("unsupported embedded native motion action '{other}'"),
        }
    }

    async fn play_animation(
        &self,
        action: &ExpressionTimelineAction,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let file = animation_file_from_params(&self.config.animation_dir, &action.params)?;
        let frames = load_animation_frames(&file)?;
        if frames.is_empty() {
            bail!(
                "animation file contains no rotation frames: {}",
                file.display()
            );
        }
        let fps = action
            .params
            .get("fps")
            .and_then(serde_json::Value::as_u64)
            .map(|fps| fps.clamp(1, 120) as u32)
            .unwrap_or_else(|| self.config.fps.max(1));
        let loops = action
            .params
            .get("loops")
            .and_then(serde_json::Value::as_u64)
            .map(|loops| loops.clamp(1, 20) as usize)
            .unwrap_or(1);
        let frame_limit = if action.duration_ms > 0 {
            Some(((action.duration_ms as u64 * fps as u64) / 1000).max(1) as usize)
        } else {
            None
        };
        self.play_animation_frames(&frames, fps, loops, frame_limit, cancel)
            .await
    }

    async fn rotation_from_params(&self, params: &serde_json::Value) -> Result<[f32; 3]> {
        if let Some(rot) = parse_rotation(params.get("rotation")) {
            return Ok(rot);
        }
        if let Some(rot) = parse_rotation(
            params
                .get("target")
                .and_then(|target| target.get("rotation")),
        ) {
            return Ok(rot);
        }
        let face = params.get("face_center").or_else(|| {
            params
                .get("target")
                .and_then(|target| target.get("face_center"))
        });
        if let Some(face) = face {
            let cx = number(face, "cx")?;
            let cy = number(face, "cy")?;
            let frame_w = number(face, "frame_w")?;
            let frame_h = number(face, "frame_h")?;
            return Ok(self.face_center_to_rotation(cx, cy, frame_w, frame_h).await);
        }
        Ok([0.0, 0.0, 0.0])
    }

    async fn face_center_to_rotation(
        &self,
        cx: f32,
        cy: f32,
        frame_w: f32,
        frame_h: f32,
    ) -> [f32; 3] {
        let half_w = (frame_w * 0.5).max(1.0);
        let half_h = (frame_h * 0.5).max(1.0);
        let center_x = half_w + self.config.face_center_offset_x;
        let center_y = half_h + self.config.face_center_offset_y;
        let raw_nx = (cx - center_x) / half_w;
        let raw_ny = (cy - center_y) / half_h;
        let theta = self.config.face_camera_roll_deg.to_radians();
        let mut nx = raw_nx * theta.cos() + raw_ny * theta.sin();
        let mut ny = -raw_nx * theta.sin() + raw_ny * theta.cos();
        if nx.abs() < self.config.face_deadzone {
            nx = 0.0;
        }
        if ny.abs() < self.config.face_deadzone {
            ny = 0.0;
        }
        nx = nx.clamp(-1.0, 1.0);
        ny = ny.clamp(-1.0, 1.0);

        let pitch = ny * self.config.face_max_pitch;
        let yaw = -nx * self.config.face_max_yaw;
        let alpha = self.config.face_smooth_alpha.clamp(0.0, 1.0);
        let mut state = self.state.lock().await;
        state.smoothed_face_rot[0] = state.smoothed_face_rot[0] * (1.0 - alpha) + pitch * alpha;
        state.smoothed_face_rot[1] = state.smoothed_face_rot[1] * (1.0 - alpha) + yaw * alpha;
        state.smoothed_face_rot[2] *= 1.0 - alpha;
        state.smoothed_face_rot
    }

    async fn play_rotation_path(
        &self,
        points: &[[f32; 3]],
        duration_ms: u32,
        apply_mapper_smoothing: bool,
        cancel: &CancellationToken,
    ) -> Result<()> {
        if points.is_empty() {
            return Ok(());
        }
        if points.len() == 1 {
            return self.set_rotation(points[0], apply_mapper_smoothing).await;
        }
        let fps = self.config.fps.max(1);
        let total_steps = ((duration_ms.max(1) as u64 * fps as u64) / 1000).max(1) as usize;
        let sleep = Duration::from_millis((1000 / fps as u64).max(1));
        for step in 0..=total_steps {
            if cancel.is_cancelled() {
                bail!("expression execution cancelled");
            }
            let progress = step as f32 / total_steps as f32;
            let rot = sample_path(points, progress);
            self.set_rotation(rot, apply_mapper_smoothing).await?;
            tokio::time::sleep(sleep).await;
        }
        Ok(())
    }

    async fn play_animation_frames(
        &self,
        frames: &[[f32; 3]],
        fps: u32,
        loops: usize,
        frame_limit: Option<usize>,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let sleep = Duration::from_millis((1000 / fps.max(1) as u64).max(1));
        let mut played = 0usize;
        for _ in 0..loops.max(1) {
            for frame in frames {
                if cancel.is_cancelled() {
                    bail!("expression execution cancelled");
                }
                if frame_limit.is_some_and(|limit| played >= limit) {
                    return Ok(());
                }
                self.set_rotation(*frame, false).await?;
                played += 1;
                tokio::time::sleep(sleep).await;
            }
        }
        Ok(())
    }

    async fn set_rotation(&self, rot: [f32; 3], apply_smoothing: bool) -> Result<()> {
        let mapper = self
            .mapper
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("neck mapper unavailable"))?;
        let mut mapper = mapper.lock().await;
        let angles = mapper.map(rot, apply_smoothing);
        drop(mapper);
        self.servo.set_angles(&angles).await
    }
}

#[tonic::async_trait]
impl ModuleLifecycle for MotionRuntime {
    fn name(&self) -> &'static str {
        "motion"
    }

    async fn init(&mut self, _ctx: &ModuleContext) -> Result<()> {
        if self.config.enabled
            && let Some(mapper) = &self.mapper
        {
            let servo_count = mapper.lock().await.servo_count();
            if servo_count != self.config.servo_count {
                warn!(
                    configured = self.config.servo_count,
                    matrix = servo_count,
                    "motion servo count differs from neck matrix output count"
                );
            }
        }
        Ok(())
    }

    async fn start(&self) -> Result<()> {
        if self.config.enabled && self.mapper.is_some() {
            let angles = vec![self.config.center_angle; self.config.servo_count];
            self.servo.set_angles(&angles).await?;
        }
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let _ = self.cancel_expression_json("motion runtime stopping").await;
        Ok(())
    }

    async fn health_check(&self) -> ModuleHealth {
        if self.config.enabled && self.mapper.is_some() {
            ModuleHealth::Healthy
        } else {
            ModuleHealth::Degraded
        }
    }
}

fn sample_path(points: &[[f32; 3]], progress: f32) -> [f32; 3] {
    let progress = progress.clamp(0.0, 1.0);
    let scaled = progress * (points.len() - 1) as f32;
    let index = scaled.floor() as usize;
    let next = (index + 1).min(points.len() - 1);
    let t = scaled - index as f32;
    [
        lerp(points[index][0], points[next][0], t),
        lerp(points[index][1], points[next][1], t),
        lerp(points[index][2], points[next][2], t),
    ]
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn scaled_amplitude(intensity: f32, base: f32, per_intensity: f32, max: f32) -> f32 {
    (base + intensity.clamp(0.0, 1.0) * per_intensity).clamp(0.0, max)
}

fn parse_rotation(value: Option<&serde_json::Value>) -> Option<[f32; 3]> {
    let values = value?.as_array()?;
    if values.len() < 3 {
        return None;
    }
    Some([
        values[0].as_f64()? as f32,
        values[1].as_f64()? as f32,
        values[2].as_f64()? as f32,
    ])
}

fn number(value: &serde_json::Value, key: &str) -> Result<f32> {
    value
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as f32)
        .with_context(|| format!("face_center.{key} must be numeric"))
}

fn animation_file_from_params(animation_dir: &Path, params: &serde_json::Value) -> Result<PathBuf> {
    let name = params
        .get("name")
        .or_else(|| params.get("file"))
        .or_else(|| params.get("animation"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("neck");
    let name = name.trim();
    if name.is_empty() {
        bail!("animation name must not be empty");
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        bail!("animation name must be a file name, not a path");
    }
    let file_name = if name.ends_with(".anim") {
        name.to_string()
    } else {
        format!("{name}.anim")
    };
    Ok(animation_dir.join(file_name))
}

fn load_animation_frames(path: &Path) -> Result<Vec<[f32; 3]>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading animation file: {}", path.display()))?;
    let mut frames = Vec::new();
    for line in content.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts = line.split_whitespace().take(3).collect::<Vec<_>>();
        if parts.len() < 3 {
            continue;
        }
        let rot = [
            parts[0]
                .parse::<f32>()
                .with_context(|| format!("parsing animation rot_x in {}", path.display()))?,
            parts[1]
                .parse::<f32>()
                .with_context(|| format!("parsing animation rot_y in {}", path.display()))?,
            parts[2]
                .parse::<f32>()
                .with_context(|| format!("parsing animation rot_z in {}", path.display()))?,
        ];
        frames.push(rot);
    }
    Ok(frames)
}

fn skipped(action: impl Into<String>, reason: impl Into<String>) -> serde_json::Value {
    json!({
        "action": action.into(),
        "reason": reason.into()
    })
}

fn expression_result(
    status: &str,
    command_id: Option<String>,
    executed_actions: Vec<String>,
    skipped_actions: Vec<serde_json::Value>,
    completion_acknowledged: bool,
) -> serde_json::Value {
    let mut action_receipts = executed_actions
        .iter()
        .map(|action| {
            json!({
                "action": action,
                "status": "completed"
            })
        })
        .collect::<Vec<_>>();
    action_receipts.extend(skipped_actions.iter().map(|item| {
        json!({
            "action": item.get("action").and_then(serde_json::Value::as_str).unwrap_or("unknown"),
            "status": "failed",
            "reason": item.get("reason").and_then(serde_json::Value::as_str).unwrap_or("failed")
        })
    }));

    json!({
        "status": status,
        "command_id": command_id,
        "completion_acknowledged": completion_acknowledged,
        "action_receipts": action_receipts,
        "executed_actions": executed_actions,
        "skipped_actions": skipped_actions,
        "safety_modifications": []
    })
}

trait ResultReason {
    fn with_reason(self, reason: &str) -> serde_json::Value;
}

impl ResultReason for serde_json::Value {
    fn with_reason(mut self, reason: &str) -> serde_json::Value {
        if let Some(object) = self.as_object_mut() {
            object.insert("reason".to_string(), json!(reason));
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_path_by_linear_segments() {
        let path = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]];
        assert_eq!(sample_path(&path, 0.0), [0.0, 0.0, 0.0]);
        assert_eq!(sample_path(&path, 0.5), [10.0, 0.0, 0.0]);
        assert_eq!(sample_path(&path, 1.0), [10.0, 10.0, 0.0]);
    }

    #[test]
    fn resolves_animation_file_names_under_animation_dir() {
        let path =
            animation_file_from_params(Path::new("assets/anim"), &json!({"name": "neck"})).unwrap();
        assert_eq!(path, PathBuf::from("assets/anim/neck.anim"));

        let path =
            animation_file_from_params(Path::new("assets/anim"), &json!({"file": "neck2.anim"}))
                .unwrap();
        assert_eq!(path, PathBuf::from("assets/anim/neck2.anim"));

        assert!(
            animation_file_from_params(Path::new("assets/anim"), &json!({"name": "../neck"}))
                .is_err()
        );
    }

    #[test]
    fn parses_python_anim_rotation_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.anim");
        std::fs::write(&path, "1\n0.0 1.0 2.0\n\n3.5 -4.0 5.25 99.0\nbad\n").unwrap();

        let frames = load_animation_frames(&path).unwrap();
        assert_eq!(frames, vec![[0.0, 1.0, 2.0], [3.5, -4.0, 5.25]]);
    }
}
