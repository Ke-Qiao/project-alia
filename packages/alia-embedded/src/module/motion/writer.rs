//! The only place that enters a servo backend. The blocking closure remains
//! owned until every entered native call has returned, including during stop.
use std::sync::Arc;
use std::time::{Duration, Instant};

use alia_core::body::{BodyBackend, BodyEnd, BodySubmissionBoundary};
use anyhow::{Context, Result, bail, ensure};
use uuid::Uuid;

use crate::config::MotionConfig;

use super::neck_mapper::NeckMapper;
use super::plan::{self, PlannedFrame};
use super::runtime::{
    ActiveWork, CommandRecord, FaceWork, Permit, QueuedCommand, Shared, bounded_reason,
};
use super::servo::{ServoBackend, ServoBoundary, ServoWrite};

pub(super) fn run(
    shared: &Shared,
    config: &MotionConfig,
    mapper: NeckMapper,
    servo: Arc<dyn ServoBackend>,
) -> Result<()> {
    let mut driver = Driver {
        shared,
        config,
        mapper,
        servo,
        instance: Uuid::new_v4(),
        previous: vec![config.center_angle; config.servo_count],
        next_write: Instant::now(),
        interval: Duration::from_secs_f64(1.0 / f64::from(config.fps)),
    };
    let startup_permit = Permit::local();
    {
        let mut state = shared.lock();
        if state.stopping {
            startup_permit.cancel("body startup cancelled by shutdown");
        }
        state.active = Some(ActiveWork::Reset(startup_permit.clone()));
    }
    let duration = if config.soft_start_enabled {
        config.soft_start_duration_ms
    } else {
        0
    };
    let startup_completed = driver.reset(duration, &startup_permit)?;
    {
        let mut state = shared.lock();
        state.active = None;
        state.ready = startup_completed && !state.stopping;
        let startup = if state.ready {
            Ok(())
        } else {
            Err("body startup cancelled".into())
        };
        shared.startup.send_replace(Some(startup));
    }
    loop {
        let work = {
            let mut state = shared.lock();
            loop {
                if state.stopping {
                    break None;
                }
                if let Some(command) = state.queue.pop_front() {
                    state.active = Some(ActiveWork::Command(command.record.clone()));
                    break Some(Work::Command(command));
                }
                if let Some(face) = state.face.take() {
                    state.active = Some(ActiveWork::Face(face.permit.clone()));
                    break Some(Work::Face(face));
                }
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
        };
        let Some(work) = work else {
            break;
        };
        match work {
            Work::Command(command) => driver.command(command)?,
            Work::Face(face) => driver.face(face)?,
        }
        shared.lock().active = None;
    }
    if config.reset_on_stop {
        // A shutdown reset is explicit local lifecycle work, after all original
        // command owners have quiesced. It never borrows an expired remote lease.
        let permit = Permit::local();
        shared.lock().active = Some(ActiveWork::Reset(permit.clone()));
        ensure!(
            driver.reset(config.reset_duration_ms, &permit)?,
            "body shutdown reset expired"
        );
        shared.lock().active = None;
    }
    Ok(())
}

enum Work {
    Command(QueuedCommand),
    Face(FaceWork),
}

struct Driver<'a> {
    shared: &'a Shared,
    config: &'a MotionConfig,
    mapper: NeckMapper,
    servo: Arc<dyn ServoBackend>,
    instance: Uuid,
    // Last submitted software targets, not measured motor positions. Known
    // channels of a partial I2C frame are retained independently.
    previous: Vec<f32>,
    next_write: Instant,
    interval: Duration,
}

impl Driver<'_> {
    fn command(&mut self, work: QueuedCommand) -> Result<()> {
        let record = work.record;
        if let Some(reason) = record.permit.reason() {
            return record.finish(BodyEnd::Cancelled { reason });
        }
        // Current software targets are known only to this writer. Expand every
        // action now, before the first write, so a late invalid animation or
        // speed expansion cannot leave half of an otherwise rejected command.
        let frames = match plan::expand(
            self.config,
            &mut self.mapper.clone(),
            &self.previous,
            &work.prepared.actions,
        ) {
            Ok(frames) => frames,
            Err(error) => {
                return record.finish(BodyEnd::Rejected {
                    reason: bounded_reason(&format!("body preflight: {error:#}")),
                });
            }
        };
        for frame in frames {
            if !self.pace(&record.permit) {
                return record.finish(BodyEnd::Cancelled {
                    reason: record
                        .permit
                        .reason()
                        .unwrap_or_else(|| "body command cancelled".into()),
                });
            }
            record.update(|progress| {
                progress.started_actions = progress.started_actions.max(frame.action as u32 + 1)
            })?;
            let write = self.submit(&frame.angles, &record.permit);
            self.record_submission(&record, &frame, &write)?;
            if let Some(error) = write.error {
                record.finish(BodyEnd::Failed {
                    reason: bounded_reason(&error),
                })?;
                bail!("body native write failed; writer sealed: {error}");
            }
            if write.cancelled || record.permit.reason().is_some() {
                record.finish(BodyEnd::Cancelled {
                    reason: record
                        .permit
                        .reason()
                        .unwrap_or_else(|| "body native submission cancelled".into()),
                })?;
                ensure!(
                    write.uncertain_channels.is_empty(),
                    "cancelled serial frame left unknown native effects; writer sealed"
                );
                return Ok(());
            }
            ensure!(
                write.fully_submitted(self.config.servo_count),
                "body backend returned incomplete submission without an error or cancellation"
            );
        }
        record.finish(BodyEnd::Submitted)
    }

    fn record_submission(
        &mut self,
        record: &CommandRecord,
        frame: &PlannedFrame,
        write: &ServoWrite,
    ) -> Result<()> {
        validate_write(write, self.config.servo_count)?;
        let backend = self.backend(write);
        let complete = write.submitted_channels.len() == self.config.servo_count
            && write.uncertain_channels.is_empty();
        self.retain_known_channels(&frame.angles, write);
        record.update(|progress| {
            progress.backend = Some(backend);
            if complete {
                progress.submitted_frames += 1;
                if frame.action_end {
                    progress.submitted_actions = frame.action as u32 + 1;
                }
            } else if !write.submitted_channels.is_empty() || !write.uncertain_channels.is_empty() {
                progress.partial_frame = Some(progress.submitted_frames);
                progress.last_partial_channels = write.submitted_channels.clone();
                progress.last_uncertain_channels = write.uncertain_channels.clone();
            }
        })
    }

    fn backend(&self, write: &ServoWrite) -> BodyBackend {
        BodyBackend {
            instance_id: self.instance,
            boundary: match write.boundary {
                ServoBoundary::SerialHostWrite => BodySubmissionBoundary::SerialHostWrite,
                ServoBoundary::I2cRegisterWrite => BodySubmissionBoundary::I2cRegisterWrite,
                ServoBoundary::Simulated => BodySubmissionBoundary::Simulated,
            },
            channels: self.config.servo_count as u32,
        }
    }

    fn retain_known_channels(&mut self, angles: &[f32], write: &ServoWrite) {
        for &channel in &write.submitted_channels {
            self.previous[channel as usize] = angles[channel as usize];
        }
    }

    fn face(&mut self, face: FaceWork) -> Result<()> {
        let frames = plan::expand(
            self.config,
            &mut self.mapper.clone(),
            &self.previous,
            &[vec![face.rotation]],
        )?;
        for frame in frames {
            if !self.pace(&face.permit) {
                break;
            }
            let write = self.submit(&frame.angles, &face.permit);
            validate_write(&write, self.config.servo_count)?;
            self.retain_known_channels(&frame.angles, &write);
            ensure!(
                write.error.is_none(),
                "local face native write failed: {}",
                write.error.as_deref().unwrap_or_default()
            );
            ensure!(
                write.uncertain_channels.is_empty(),
                "local face write left unknown native effects"
            );
            if write.cancelled {
                break;
            }
            ensure!(
                write.fully_submitted(self.config.servo_count),
                "local face native write has incomplete evidence"
            );
        }
        Ok(())
    }

    fn reset(&mut self, minimum_duration_ms: u32, permit: &Permit) -> Result<bool> {
        let target = vec![self.config.center_angle; self.config.servo_count];
        self.mapper.validate_angles(&target)?;
        let max_delta = self
            .previous
            .iter()
            .zip(&target)
            .map(|(old, new)| (old - new).abs())
            .fold(0.0_f32, f32::max);
        let speed_steps = (max_delta
            / (self.config.max_servo_speed_deg_per_sec / self.config.fps as f32))
            .ceil() as usize;
        let duration_steps =
            (u64::from(minimum_duration_ms) * u64::from(self.config.fps)).div_ceil(1000) as usize;
        let steps = speed_steps.max(duration_steps).max(1);
        ensure!(
            steps <= (self.config.fps as usize * 30).min(3600),
            "body reset exceeds safe expansion bound"
        );
        let previous = self.previous.clone();
        for step in 1..=steps {
            if !self.pace(permit) {
                return Ok(false);
            }
            let angles: Vec<_> = previous
                .iter()
                .zip(&target)
                .map(|(old, new)| old + (new - old) * step as f32 / steps as f32)
                .collect();
            let write = self.submit(&angles, permit);
            validate_write(&write, self.config.servo_count)?;
            self.retain_known_channels(&angles, &write);
            ensure!(
                write.error.is_none(),
                "body reset native write failed: {}",
                write.error.as_deref().unwrap_or_default()
            );
            ensure!(
                write.uncertain_channels.is_empty(),
                "body reset left unknown native effects"
            );
            if write.cancelled {
                return Ok(false);
            }
            ensure!(
                write.fully_submitted(self.config.servo_count),
                "body reset native write has incomplete evidence"
            );
        }
        Ok(true)
    }

    fn submit(&mut self, angles: &[f32], permit: &Permit) -> ServoWrite {
        let result = self
            .servo
            .submit_frame(angles, &|| permit.reason().is_some());
        // Never catch up after slow I/O or jump directly between commands. The
        // speed budget is global across body, face and lifecycle reset frames.
        self.next_write = Instant::now() + self.interval;
        result
    }

    fn pace(&self, permit: &Permit) -> bool {
        let mut state = self.shared.lock();
        loop {
            if permit.reason().is_some() {
                return false;
            }
            let now = Instant::now();
            if now >= self.next_write {
                return true;
            }
            let result = self
                .shared
                .wake
                .wait_timeout(state, self.next_write - now)
                .unwrap_or_else(|error| error.into_inner());
            state = result.0;
        }
    }
}

fn validate_write(write: &ServoWrite, count: usize) -> Result<()> {
    let mut seen = vec![false; count];
    for &channel in write
        .submitted_channels
        .iter()
        .chain(&write.uncertain_channels)
    {
        let entry = seen
            .get_mut(channel as usize)
            .context("body backend reported an invalid channel")?;
        ensure!(!*entry, "body backend duplicated native channel evidence");
        *entry = true;
    }
    Ok(())
}
