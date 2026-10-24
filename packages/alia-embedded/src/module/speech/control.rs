//! Explicit session admission; this does not open a microphone or command R1.
use std::sync::{Arc, Mutex};

use alia_proto::capture::{CaptureRequest, CaptureState};
use anyhow::{Result, anyhow, ensure};
use tokio::sync::{mpsc, oneshot};

use crate::binding::BindingData;

pub struct CaptureControl {
    state: Mutex<State>,
}

struct State {
    snapshot: CaptureState,
    commands: Option<mpsc::Sender<Command>>,
}

pub(super) struct Command {
    pub request: CaptureRequest,
    pub start: bool,
    pub binding: BindingData,
    pub token: String,
    pub reply: oneshot::Sender<Result<CaptureState>>,
}

pub(super) struct RuntimeControl {
    pub controller: Arc<CaptureControl>,
    pub commands: mpsc::Receiver<Command>,
}

impl CaptureControl {
    pub(super) fn new(device_boot_id: String) -> Self {
        Self {
            state: Mutex::new(State {
                snapshot: CaptureState {
                    device_boot_id,
                    generation: 0,
                    active: false,
                    available: false,
                },
                commands: None,
            }),
        }
    }

    pub fn inspect(&self) -> Result<CaptureState> {
        Ok(self
            .state
            .lock()
            .map_err(|_| anyhow!("capture control lock poisoned"))?
            .snapshot
            .clone())
    }

    pub(crate) fn enqueue(
        &self,
        request: CaptureRequest,
        start: bool,
        binding: BindingData,
        token: String,
    ) -> Result<oneshot::Receiver<Result<CaptureState>>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow!("capture control lock poisoned"))?;
        validate(&state.snapshot, &request)?;
        let (reply, result) = oneshot::channel();
        state
            .commands
            .as_ref()
            .ok_or_else(|| anyhow!("capture pipeline is unavailable"))?
            .try_send(Command {
                request,
                start,
                binding,
                token,
                reply,
            })
            .map_err(|_| anyhow!("capture command queue is full or stopped"))?;
        Ok(result)
    }

    pub(super) fn attach(self: &Arc<Self>) -> Result<RuntimeControl> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("capture control lock poisoned"))?;
        ensure!(state.commands.is_none(), "capture control already attached");
        advance(&mut state.snapshot)?;
        let (commands, receiver) = mpsc::channel(8);
        state.commands = Some(commands);
        state.snapshot.available = true;
        Ok(RuntimeControl {
            controller: self.clone(),
            commands: receiver,
        })
    }

    pub(super) fn validate(&self, request: &CaptureRequest) -> Result<()> {
        validate(&self.inspect()?, request)
    }

    pub(super) fn begin(&self) -> Result<CaptureState> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("capture control lock poisoned"))?;
        ensure!(
            state.snapshot.available && !state.snapshot.active,
            "capture session is already active or unavailable"
        );
        advance(&mut state.snapshot)?;
        state.snapshot.active = true;
        Ok(state.snapshot.clone())
    }

    pub(super) fn finish(&self) -> Result<CaptureState> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("capture control lock poisoned"))?;
        if state.snapshot.active {
            // Close admission even if the generation counter is exhausted.
            state.snapshot.active = false;
            if let Err(error) = advance(&mut state.snapshot) {
                state.snapshot.available = false;
                return Err(error);
            }
        }
        Ok(state.snapshot.clone())
    }

    pub(super) fn fail_closed(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.snapshot.available = false;
    }
}

fn validate(state: &CaptureState, request: &CaptureRequest) -> Result<()> {
    ensure!(
        state.device_boot_id == request.device_boot_id,
        "device boot changed"
    );
    ensure!(
        state.generation == request.expected_generation,
        "capture generation changed; inspect before retrying"
    );
    ensure!(state.available, "capture pipeline is unavailable");
    Ok(())
}

fn advance(state: &mut CaptureState) -> Result<()> {
    state.generation = state
        .generation
        .checked_add(1)
        .ok_or_else(|| anyhow!("capture generation exhausted"))?;
    Ok(())
}

impl Drop for RuntimeControl {
    fn drop(&mut self) {
        let mut state = self
            .controller
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.commands = None;
        state.snapshot.active = false;
        state.snapshot.available = false;
        let _ = advance(&mut state.snapshot);
    }
}
