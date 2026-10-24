//! The only place that enters a servo backend. The blocking closure remains
//! owned until every entered native call has returned, including during stop.
use std::sync::Arc;
use std::time::{Duration, Instant};

use alia_core::embodiment::motion::{MotionBackend, MotionEnd, MotionSubmissionBoundary};
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
        previous: None,
        next_write: Instant::now(),
        interval: Duration::from_secs_f64(1.0 / f64::from(config.fps)),
    };
    let startup_permit = Permit::local();
    {
        let mut state = shared.lock();
        if state.stopping {
            startup_permit.cancel("motion startup cancelled by shutdown");
        }
        state.active = Some(ActiveWork::Startup(startup_permit.clone()));
    }
    let startup_completed = driver.center_at_startup(&startup_permit)?;
    {
        let mut state = shared.lock();
        state.active = None;
        state.ready = startup_completed && !state.stopping;
        let startup = if state.ready {
            Ok(())
        } else {
            Err("motion startup cancelled".into())
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
    // Stop preserves the last submitted output. No new motion or disable is
    // admitted after the original native owner has quiesced.
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
    previous: Option<Vec<f32>>,
    next_write: Instant,
    interval: Duration,
}

impl Driver<'_> {
    fn command(&mut self, work: QueuedCommand) -> Result<()> {
        let record = work.record;
        if let Some(reason) = record.permit.reason() {
            return record.finish(MotionEnd::Cancelled { reason });
        }
        // Current software targets are known only to this writer. Expand every
        // action now, before the first write, so a late invalid animation or
        // speed expansion cannot leave half of an otherwise rejected command.
        let frames = match plan::expand(
            self.config,
            &mut self.mapper.clone(),
            self.previous
                .as_deref()
                .context("motion startup has no complete submitted target")?,
            &work.prepared.actions,
        ) {
            Ok(frames) => frames,
            Err(error) => {
                return record.finish(MotionEnd::Rejected {
                    reason: bounded_reason(&format!("motion preflight: {error:#}")),
                });
            }
        };
        for frame in frames {
            if !self.pace(&record.permit) {
                return record.finish(MotionEnd::Cancelled {
                    reason: record
                        .permit
                        .reason()
                        .unwrap_or_else(|| "motion command cancelled".into()),
                });
            }
            record.update(|progress| {
                progress.started_actions = progress.started_actions.max(frame.action as u32 + 1)
            })?;
            let write = self.submit(&frame.angles, &record.permit);
            self.record_submission(&record, &frame, &write)?;
            if let Some(error) = write.error {
                record.finish(MotionEnd::Failed {
                    reason: bounded_reason(&error),
                })?;
                bail!("motion native write failed; writer sealed: {error}");
            }
            if write.cancelled || record.permit.reason().is_some() {
                record.finish(MotionEnd::Cancelled {
                    reason: record
                        .permit
                        .reason()
                        .unwrap_or_else(|| "motion native submission cancelled".into()),
                })?;
                ensure!(
                    write.uncertain_channels.is_empty(),
                    "cancelled serial frame left unknown native effects; writer sealed"
                );
                return Ok(());
            }
            ensure!(
                write.fully_submitted(self.config.servo_count),
                "motion backend returned incomplete submission without an error or cancellation"
            );
        }
        record.finish(MotionEnd::Submitted)
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

    fn backend(&self, write: &ServoWrite) -> MotionBackend {
        MotionBackend {
            instance_id: self.instance,
            boundary: match write.boundary {
                ServoBoundary::SerialHostWrite => MotionSubmissionBoundary::SerialHostWrite,
                ServoBoundary::I2cRegisterWrite => MotionSubmissionBoundary::I2cRegisterWrite,
                ServoBoundary::Simulated => MotionSubmissionBoundary::Simulated,
            },
            channels: self.config.servo_count as u32,
        }
    }

    fn retain_known_channels(&mut self, angles: &[f32], write: &ServoWrite) {
        if let Some(previous) = &mut self.previous {
            for &channel in &write.submitted_channels {
                previous[channel as usize] = angles[channel as usize];
            }
        } else if write.fully_submitted(self.config.servo_count) {
            self.previous = Some(angles.to_vec());
        }
    }

    fn face(&mut self, face: FaceWork) -> Result<()> {
        let frames = plan::expand(
            self.config,
            &mut self.mapper.clone(),
            self.previous
                .as_deref()
                .context("motion startup has no complete submitted target")?,
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

    fn center_at_startup(&mut self, permit: &Permit) -> Result<bool> {
        ensure!(
            self.config.calibration_confirmed,
            "motion startup requires confirmed mechanism calibration"
        );
        let target = vec![self.config.center_angle; self.config.servo_count];
        self.mapper.validate_angles(&target)?;
        if !self.pace(permit) {
            return Ok(false);
        }
        // The physical starting angle is unknown. Calibration authorizes this
        // one center target; no interpolation can prove the first motion speed.
        let write = self.submit(&target, permit);
        validate_write(&write, self.config.servo_count)?;
        self.retain_known_channels(&target, &write);
        ensure!(
            write.error.is_none(),
            "motion startup native write failed: {}",
            write.error.as_deref().unwrap_or_default()
        );
        ensure!(
            write.uncertain_channels.is_empty(),
            "motion startup left unknown native effects"
        );
        if write.cancelled || permit.reason().is_some() {
            return Ok(false);
        }
        ensure!(
            write.fully_submitted(self.config.servo_count),
            "motion startup native write has incomplete evidence"
        );
        Ok(true)
    }

    fn submit(&mut self, angles: &[f32], permit: &Permit) -> ServoWrite {
        let result = self
            .servo
            .submit_frame(angles, &|| permit.reason().is_some());
        // Never catch up after slow I/O or jump directly between commands. The
        // speed budget applies between submitted motion and face targets after
        // the startup target, never from an assumed physical starting angle.
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
            .context("motion backend reported an invalid channel")?;
        ensure!(!*entry, "motion backend duplicated native channel evidence");
        *entry = true;
    }
    Ok(())
}
