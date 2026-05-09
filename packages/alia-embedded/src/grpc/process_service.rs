use std::sync::Arc;

use tonic::{Request, Response, Status};

use alia_proto::process::process_manager_server::ProcessManager as ProcessManagerTrait;
use alia_proto::process::{
    ListProcessesRequest, ListProcessesResponse, ProcessInfo, ProcessOutput, RestartProcessRequest,
    RestartProcessResponse, StartProcessRequest, StartProcessResponse, StopProcessRequest,
    StopProcessResponse, StreamOutputRequest,
};

use crate::process::{self, ProcessManager};

pub struct ProcessManagerService {
    manager: Arc<ProcessManager>,
}

impl ProcessManagerService {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self { manager }
    }
}

#[tonic::async_trait]
impl ProcessManagerTrait for ProcessManagerService {
    async fn start_process(
        &self,
        request: Request<StartProcessRequest>,
    ) -> Result<Response<StartProcessResponse>, Status> {
        let req = request.into_inner();
        match self.manager.start(&req.name).await {
            Ok(pid) => Ok(Response::new(StartProcessResponse {
                success: true,
                message: format!("started with pid {pid}"),
                pid,
            })),
            Err(e) => Ok(Response::new(StartProcessResponse {
                success: false,
                message: e.to_string(),
                pid: 0,
            })),
        }
    }

    async fn stop_process(
        &self,
        request: Request<StopProcessRequest>,
    ) -> Result<Response<StopProcessResponse>, Status> {
        let req = request.into_inner();
        match self.manager.stop(&req.name, req.force).await {
            Ok(()) => Ok(Response::new(StopProcessResponse {
                success: true,
                message: "stopped".to_string(),
            })),
            Err(e) => Ok(Response::new(StopProcessResponse {
                success: false,
                message: e.to_string(),
            })),
        }
    }

    async fn restart_process(
        &self,
        request: Request<RestartProcessRequest>,
    ) -> Result<Response<RestartProcessResponse>, Status> {
        let req = request.into_inner();
        match self.manager.restart(&req.name).await {
            Ok(pid) => Ok(Response::new(RestartProcessResponse {
                success: true,
                message: format!("restarted with pid {pid}"),
                pid,
            })),
            Err(e) => Ok(Response::new(RestartProcessResponse {
                success: false,
                message: e.to_string(),
                pid: 0,
            })),
        }
    }

    async fn list_processes(
        &self,
        _request: Request<ListProcessesRequest>,
    ) -> Result<Response<ListProcessesResponse>, Status> {
        let snapshots = self.manager.list().await;
        let processes = snapshots
            .into_iter()
            .map(|s| ProcessInfo {
                name: s.name,
                state: process_state_to_proto(s.state) as i32,
                pid: s.pid,
                uptime_secs: s.uptime_secs,
                restart_count: s.restart_count,
            })
            .collect();

        Ok(Response::new(ListProcessesResponse { processes }))
    }

    type StreamOutputStream = tokio_stream::wrappers::ReceiverStream<Result<ProcessOutput, Status>>;

    async fn stream_output(
        &self,
        request: Request<StreamOutputRequest>,
    ) -> Result<Response<Self::StreamOutputStream>, Status> {
        let req = request.into_inner();
        let name_filter = req.name.clone();

        let mut rx = self.manager.subscribe_output();
        let (tx, stream_rx) = tokio::sync::mpsc::channel(128);

        tokio::spawn(async move {
            while let Ok(line) = rx.recv().await {
                if !name_filter.is_empty() && line.name != name_filter {
                    continue;
                }
                let output = ProcessOutput {
                    name: line.name,
                    channel: match line.channel {
                        process::OutputChannel::Stdout => 0,
                        process::OutputChannel::Stderr => 1,
                    },
                    line: line.line,
                    timestamp_us: line.timestamp_us,
                };
                if tx.send(Ok(output)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            stream_rx,
        )))
    }
}

fn process_state_to_proto(state: process::ProcessState) -> i32 {
    match state {
        process::ProcessState::Stopped => 1,
        process::ProcessState::Starting => 2,
        process::ProcessState::Running => 3,
        process::ProcessState::Stopping => 4,
        process::ProcessState::Failed => 5,
    }
}
