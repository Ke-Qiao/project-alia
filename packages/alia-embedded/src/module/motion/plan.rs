use super::neck_mapper::NeckMapper;
use crate::config::MotionConfig;
use alia_core::body::{BodyCommand, BodyMotion, BodyTarget, MAX_BODY_PAYLOAD_BYTES};
use anyhow::{Context, Result, ensure};
use std::io::Read;

/// Preparation performs bounded file reads only. It never claims an identity or
/// touches hardware; submit rechecks all authority after this work has joined.
#[derive(Debug)]
pub struct PreparedBodyCommand {
    pub(super) command: BodyCommand,
    pub(super) signed_bytes: Vec<u8>,
    pub(super) actions: Vec<Vec<[f32; 3]>>,
}

pub(super) fn prepare(
    config: &MotionConfig,
    command: BodyCommand,
    signed_bytes: Vec<u8>,
) -> Result<PreparedBodyCommand> {
    validate_canonical(&command, &signed_bytes)?;
    let mut actions = Vec::new();
    for action in &command.actions {
        let (max_intensity, max_duration) = match action.motion {
            BodyMotion::SmallNod => (0.45, 900),
            BodyMotion::SmallBow => (0.35, 1200),
            BodyMotion::LookAt { .. } => (0.45, 1200),
            BodyMotion::Animation { .. } => (0.45, 5000),
        };
        ensure!(
            action.intensity <= max_intensity && action.duration_ms <= max_duration,
            "action exceeds device safety capability"
        );
        let steps = (u64::from(action.duration_ms) * u64::from(config.fps))
            .div_ceil(1000)
            .max(1) as usize;
        let points = match &action.motion {
            BodyMotion::SmallNod => {
                let amplitude = (3.0 + action.intensity * 20.0).min(12.0);
                vec![
                    [0.0; 3],
                    [amplitude, 0.0, 0.0],
                    [-amplitude * 0.55, 0.0, 0.0],
                    [0.0; 3],
                ]
            }
            BodyMotion::SmallBow => {
                let amplitude = (4.0 + action.intensity * 22.0).min(12.0);
                vec![
                    [0.0; 3],
                    [amplitude, 0.0, 0.0],
                    [amplitude * 0.85, 0.0, 0.0],
                    [0.0; 3],
                ]
            }
            BodyMotion::LookAt { target } => vec![
                [0.0; 3],
                match target {
                    BodyTarget::Rotation { degrees } => *degrees,
                    BodyTarget::FaceCenter {
                        cx,
                        cy,
                        width,
                        height,
                    } => face_rotation(config, *cx, *cy, *width, *height)?,
                },
            ],
            BodyMotion::Animation { file, fps, loops } => {
                let frames = read_animation(config, file)?;
                let count = frames
                    .len()
                    .checked_mul(*loops as usize)
                    .context("animation count overflow")?;
                ensure!(count <= 600, "animation exceeds 600 source frames");
                ensure!(
                    (count as u64 * 1000).div_ceil(u64::from(*fps))
                        <= u64::from(action.duration_ms),
                    "animation duration does not fit declared action duration"
                );
                // Preserve source cadence at the fixed writer rate, holding the
                // final frame for unused declared duration. Always include the
                // tail, even when a source frame spans less than one writer tick.
                let all: Vec<_> = (0..*loops).flat_map(|_| frames.iter().copied()).collect();
                let last = all.len() - 1;
                let rotations = (1..=steps)
                    .map(|step| {
                        let source_frame = step as f32 * *fps as f32 / config.fps as f32;
                        let progress = if step == steps || last == 0 {
                            1.0
                        } else {
                            (source_frame / last as f32).min(1.0)
                        };
                        sample_path(&all, progress)
                    })
                    .collect();
                actions.push(rotations);
                continue;
            }
        };
        actions.push(
            (1..=steps)
                .map(|step| sample_path(&points, step as f32 / steps as f32))
                .collect(),
        );
    }
    Ok(PreparedBodyCommand {
        command,
        signed_bytes,
        actions,
    })
}

pub(super) fn validate_canonical(command: &BodyCommand, canonical: &[u8]) -> Result<()> {
    command.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        !canonical.is_empty() && canonical.len() <= MAX_BODY_PAYLOAD_BYTES,
        "invalid body intent size"
    );
    ensure!(
        canonical == serde_json::to_vec(command)?,
        "body canonical bytes do not match the command"
    );
    Ok(())
}

fn read_animation(config: &MotionConfig, name: &str) -> Result<Vec<[f32; 3]>> {
    let directory = config
        .animation_dir
        .canonicalize()
        .context("resolving animation directory")?;
    let name = if name.ends_with(".anim") {
        name.to_owned()
    } else {
        format!("{name}.anim")
    };
    let path = directory
        .join(name)
        .canonicalize()
        .context("resolving animation")?;
    ensure!(
        path.parent() == Some(directory.as_path()),
        "animation must stay inside configured directory"
    );
    let file = std::fs::File::open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "animation must be a regular file"
    );
    let mut content = String::new();
    file.take(65_537).read_to_string(&mut content)?;
    ensure!(content.len() <= 65_536, "animation exceeds 64 KiB");
    let mut frames = Vec::new();
    for line in content
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
    {
        let values = line
            .split_whitespace()
            .map(str::parse::<f32>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ensure!(
            values.len() == 3 && values.iter().all(|v| v.is_finite() && v.abs() <= 180.0),
            "animation requires three finite bounded angles per frame"
        );
        ensure!(frames.len() < 600, "animation exceeds 600 frames");
        frames.push([values[0], values[1], values[2]]);
    }
    ensure!(!frames.is_empty(), "animation contains no frames");
    Ok(frames)
}

pub(super) fn face_rotation(
    config: &MotionConfig,
    cx: f32,
    cy: f32,
    width: f32,
    height: f32,
) -> Result<[f32; 3]> {
    ensure!(
        [cx, cy, width, height].iter().all(|v| v.is_finite())
            && width > 0.0
            && height > 0.0
            && (0.0..=width).contains(&cx)
            && (0.0..=height).contains(&cy),
        "invalid face coordinates"
    );
    let half_w = (width * 0.5).max(1.0);
    let half_h = (height * 0.5).max(1.0);
    let x = (cx - half_w - config.face_center_offset_x) / half_w;
    let y = (cy - half_h - config.face_center_offset_y) / half_h;
    let theta = config.face_camera_roll_deg.to_radians();
    let mut nx = x * theta.cos() + y * theta.sin();
    let mut ny = -x * theta.sin() + y * theta.cos();
    if nx.abs() < config.face_deadzone {
        nx = 0.0;
    }
    if ny.abs() < config.face_deadzone {
        ny = 0.0;
    }
    Ok([
        ny.clamp(-1.0, 1.0) * config.face_max_pitch,
        -nx.clamp(-1.0, 1.0) * config.face_max_yaw,
        0.0,
    ])
}

pub(super) fn sample_path(points: &[[f32; 3]], t: f32) -> [f32; 3] {
    let scaled = t.clamp(0.0, 1.0) * (points.len() - 1) as f32;
    let i = scaled.floor() as usize;
    let j = (i + 1).min(points.len() - 1);
    std::array::from_fn(|axis| {
        points[i][axis] + (points[j][axis] - points[i][axis]) * (scaled - i as f32)
    })
}

pub(super) struct PlannedFrame {
    pub angles: Vec<f32>,
    pub action: usize,
    pub action_end: bool,
}

/// Expand the whole plan before its first write. Mapping/calibration is unchanged;
/// fixed-rate speed limiting may lengthen a path, but cannot run without a bound.
pub(super) fn expand(
    config: &MotionConfig,
    mapper: &mut NeckMapper,
    previous: &[f32],
    actions: &[Vec<[f32; 3]>],
) -> Result<Vec<PlannedFrame>> {
    let mut last = previous.to_vec();
    mapper.validate_angles(&last)?;
    let mut frames = Vec::new();
    let limit = (config.fps as usize * 30).min(3600);
    for (action, rotations) in actions.iter().enumerate() {
        for rotation in rotations {
            let angles = mapper.map(*rotation, false);
            mapper.validate_angles(&angles)?;
            let max_delta = last
                .iter()
                .zip(&angles)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            let steps = (max_delta / (config.max_servo_speed_deg_per_sec / config.fps as f32))
                .ceil()
                .max(1.0) as usize;
            ensure!(
                frames.len().checked_add(steps).is_some_and(|n| n <= limit),
                "speed-limited body plan exceeds 30 seconds / 3600 frames"
            );
            for step in 1..=steps {
                frames.push(PlannedFrame {
                    angles: last
                        .iter()
                        .zip(&angles)
                        .map(|(a, b)| a + (b - a) * step as f32 / steps as f32)
                        .collect(),
                    action,
                    action_end: false,
                });
            }
            last = angles;
        }
        frames
            .last_mut()
            .context("empty prepared action")?
            .action_end = true;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alia_core::body::{
        BodyAction, BodyCommandIdentity, BodyControlLease, BodyPriority, BodyScope,
    };
    use uuid::Uuid;

    fn animation(file: &str, fps: u32, loops: u32, duration_ms: u32) -> BodyCommand {
        BodyCommand {
            identity: BodyCommandIdentity {
                lease: BodyControlLease {
                    device_boot_id: Uuid::new_v4(),
                    server_instance_id: Uuid::new_v4(),
                    generation: 1,
                },
                sequence: 1,
            },
            plan_id: Uuid::new_v4(),
            source_command_id: "animation-test".into(),
            owner_id: Uuid::new_v4(),
            scope: BodyScope::Nonvoice {
                scope_id: Uuid::new_v4(),
            },
            priority: BodyPriority::Normal,
            interruptible: true,
            actions: vec![BodyAction {
                duration_ms,
                intensity: 0.1,
                motion: BodyMotion::Animation {
                    file: file.into(),
                    fps,
                    loops,
                },
            }],
        }
    }

    fn prepared(config: &MotionConfig, command: BodyCommand) -> Result<PreparedBodyCommand> {
        let bytes = serde_json::to_vec(&command)?;
        prepare(config, command, bytes)
    }

    #[test]
    fn validates_every_animation_frame_before_returning_any_plan() {
        let directory = tempfile::tempdir().unwrap();
        let config = MotionConfig {
            animation_dir: directory.path().into(),
            ..MotionConfig::default()
        };
        std::fs::write(
            directory.path().join("bad.anim"),
            "frames\n0 0 0\n1 2 NaN\n",
        )
        .unwrap();
        let mut command = animation("bad", 30, 1, 100);
        command.actions.insert(
            0,
            BodyAction {
                duration_ms: 100,
                intensity: 0.1,
                motion: BodyMotion::SmallNod,
            },
        );
        assert!(
            prepared(&config, command).is_err(),
            "a valid early action cannot hide an invalid late action"
        );
        std::fs::write(directory.path().join("bad.anim"), "frames\n1 2 3 4\n").unwrap();
        assert!(prepared(&config, animation("bad", 30, 1, 100)).is_err());
    }

    #[test]
    fn rejects_animation_file_size_frame_count_loops_and_duration_overflow() {
        let directory = tempfile::tempdir().unwrap();
        let config = MotionConfig {
            animation_dir: directory.path().into(),
            ..MotionConfig::default()
        };
        let path = directory.path().join("large.anim");
        std::fs::write(&path, " ".repeat(65_537)).unwrap();
        assert!(prepared(&config, animation("large", 120, 1, 5_000)).is_err());
        std::fs::write(&path, format!("frames\n{}", "0 0 0\n".repeat(601))).unwrap();
        assert!(prepared(&config, animation("large", 120, 1, 5_000)).is_err());
        std::fs::write(&path, format!("frames\n{}", "0 0 0\n".repeat(31))).unwrap();
        assert!(prepared(&config, animation("large", 120, 20, 5_000)).is_err());
        std::fs::write(&path, "frames\n0 0 0\n1 1 1\n").unwrap();
        assert!(prepared(&config, animation("large", 1, 1, 100)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_and_non_regular_animation_files() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside.anim"), "frames\n0 0 0\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.anim"),
            directory.path().join("link.anim"),
        )
        .unwrap();
        std::fs::create_dir(directory.path().join("directory.anim")).unwrap();
        let config = MotionConfig {
            animation_dir: directory.path().into(),
            ..MotionConfig::default()
        };
        assert!(prepared(&config, animation("link", 30, 1, 100)).is_err());
        assert!(prepared(&config, animation("directory", 30, 1, 100)).is_err());
    }

    #[test]
    fn preserves_animation_cadence_and_complete_tail_instead_of_stretching_to_deadline() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("path.anim"),
            "frames\n0 0 0\n10 0 0\n20 0 0\n",
        )
        .unwrap();
        let config = MotionConfig {
            animation_dir: directory.path().into(),
            fps: 20,
            ..MotionConfig::default()
        };
        let plan = prepared(&config, animation("path", 10, 1, 400)).unwrap();
        let path = &plan.actions[0];
        assert_eq!(path.len(), 8);
        assert_eq!(path[0], [5.0, 0.0, 0.0]);
        assert_eq!(path[1], [10.0, 0.0, 0.0]);
        assert!(
            path[3..]
                .iter()
                .all(|rotation| *rotation == [20.0, 0.0, 0.0])
        );
    }

    #[test]
    fn face_coordinates_must_be_finite_and_inside_the_image() {
        let config = MotionConfig::default();
        assert!(face_rotation(&config, f32::NAN, 1.0, 100.0, 100.0).is_err());
        assert!(face_rotation(&config, 1.0, 1.0, 0.0, 100.0).is_err());
        assert!(face_rotation(&config, 101.0, 1.0, 100.0, 100.0).is_err());
    }
}
