//! The console thread is the sole native writer. Cancelling a delivery seals
//! admission immediately, but its checkpoint waits for any entered write call.
use super::*;
use alia_core::delivery::ObservationEnd;
use alia_core::text_delivery::{
    MAX_TEXT_BYTES, MAX_TEXT_CHUNK_BYTES, MAX_TEXT_CHUNKS, TextChunk, TextFrame, TextIdentity,
    TextProgress, TextWriteBoundary,
};
use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use tokio::sync::Notify;

const RETAINED_WRITERS: usize = 64;

#[derive(Default)]
pub(super) struct TextWriters {
    writers: StdMutex<VecDeque<Arc<TextWriter>>>,
}
impl TextWriters {
    pub(super) fn is_terminal(&self, identity: TextIdentity) -> bool {
        self.writers
            .lock()
            .unwrap()
            .iter()
            .find(|writer| writer.snapshot().identity == identity)
            .is_some_and(|writer| writer.snapshot().observation_end.is_some())
    }
    fn open(&self, identity: TextIdentity) -> anyhow::Result<Arc<TextWriter>> {
        let mut writers = self.writers.lock().unwrap();
        ensure!(
            !writers
                .iter()
                .any(|writer| writer.snapshot().identity == identity),
            "text identity was already admitted"
        );
        ensure!(
            writers
                .iter()
                .all(|writer| writer.snapshot().observation_end.is_some()),
            "CLI already has an active text writer"
        );
        while writers.len() >= RETAINED_WRITERS {
            writers.pop_front();
        }
        let writer = Arc::new(TextWriter {
            state: StdMutex::new(WriterState {
                progress: TextProgress {
                    identity,
                    receipt_sequence: 1,
                    accepted_utf8_bytes: 0,
                    written_utf8_bytes: Some(0),
                    write_boundary: Some(TextWriteBoundary::StdoutWrite),
                    end_accepted: false,
                    observation_end: None,
                },
                chunks: 0,
                entered: false,
                cancellation: None,
            }),
            changed: Notify::new(),
        });
        writers.push_back(writer.clone());
        Ok(writer)
    }
    pub(super) async fn cancel(
        &self,
        identity: TextIdentity,
        reason: &str,
    ) -> anyhow::Result<TextProgress> {
        let writer = self
            .writers
            .lock()
            .unwrap()
            .iter()
            .find(|writer| writer.snapshot().identity == identity)
            .cloned()
            .context("original CLI writer is unavailable")?;
        writer.seal(reason);
        Ok(writer.quiescent().await)
    }
    pub(super) fn stop(&self) {
        for writer in self.writers.lock().unwrap().iter() {
            writer.seal("CLI stopped");
        }
    }
}

struct WriterState {
    progress: TextProgress,
    chunks: u64,
    /// Includes commands queued to the native owner. A queue ACK is not a write.
    entered: bool,
    cancellation: Option<String>,
}
struct TextWriter {
    state: StdMutex<WriterState>,
    changed: Notify,
}
impl TextWriter {
    fn snapshot(&self) -> TextProgress {
        self.state.lock().unwrap().progress.clone()
    }
    fn seal(&self, reason: &str) {
        let mut state = self.state.lock().unwrap();
        if state.progress.observation_end.is_none() {
            state.cancellation.get_or_insert_with(|| reason.to_owned());
            if !state.entered {
                finish_cancel(&mut state);
            }
        }
        drop(state);
        self.changed.notify_waiters();
    }
    async fn quiescent(&self) -> TextProgress {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock().unwrap();
                if !state.entered {
                    return state.progress.clone();
                }
            }
            changed.await;
        }
    }
    fn admit(&self, operation: &TextOperation) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap();
        ensure!(
            !state.entered
                && state.progress.observation_end.is_none()
                && state.cancellation.is_none(),
            "CLI writer admission is closed"
        );
        match operation {
            TextOperation::Start => ensure!(
                state.chunks == 0 && state.progress.accepted_utf8_bytes == 0,
                "text Start repeated"
            ),
            TextOperation::Chunk(chunk) => {
                ensure!(
                    !chunk.text.is_empty()
                        && chunk.text.len() <= MAX_TEXT_CHUNK_BYTES
                        && state.chunks < MAX_TEXT_CHUNKS
                        && state
                            .progress
                            .accepted_utf8_bytes
                            .checked_add(chunk.text.len() as u64)
                            .is_some_and(|bytes| bytes <= MAX_TEXT_BYTES),
                    "CLI text exceeds the bounded stream contract"
                );
                // Model output is data, not terminal control instructions. Keep
                // admitted bytes unchanged so receipt offsets remain exact.
                ensure!(
                    !chunk
                        .text
                        .chars()
                        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
                    "CLI text contains terminal control characters"
                );
                ensure!(
                    !state.progress.end_accepted
                        && chunk.sequence == state.chunks
                        && chunk.utf8_offset == state.progress.accepted_utf8_bytes,
                    "CLI text chunk is not contiguous"
                );
                state.chunks += 1;
                state.progress.accepted_utf8_bytes += chunk.text.len() as u64;
            }
            TextOperation::End { chunks, utf8_bytes } => {
                ensure!(
                    !state.progress.end_accepted
                        && *chunks == state.chunks
                        && *utf8_bytes == state.progress.accepted_utf8_bytes,
                    "CLI End does not match accepted chunks"
                );
                state.progress.end_accepted = true;
            }
        }
        state.entered = true;
        state.progress.receipt_sequence += 1;
        Ok(())
    }
    fn cancelled(&self) -> bool {
        self.state.lock().unwrap().cancellation.is_some()
    }
    fn written(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap();
        *state.progress.written_utf8_bytes.as_mut().unwrap() += bytes as u64;
        state.progress.receipt_sequence += 1;
    }
    fn finish(&self, operation: &TextOperation, error: Option<&io::Error>) -> TextProgress {
        let mut state = self.state.lock().unwrap();
        state.entered = false;
        if let Some(error) = error {
            state.progress.observation_end = Some(ObservationEnd::Failed {
                reason: format!("stdout write failed: {error}"),
            });
        } else if state.cancellation.is_some() {
            finish_cancel(&mut state);
        } else if matches!(operation, TextOperation::End { .. }) {
            state.progress.observation_end = Some(ObservationEnd::SubmissionFinished);
        }
        state.progress.receipt_sequence += 1;
        let progress = state.progress.clone();
        drop(state);
        self.changed.notify_waiters();
        progress
    }
}
fn finish_cancel(state: &mut WriterState) {
    state.progress.observation_end = Some(ObservationEnd::Cancelled {
        reason: state.cancellation.clone().unwrap(),
    });
    state.progress.receipt_sequence += 1;
}

enum TextOperation {
    Start,
    Chunk(TextChunk),
    End { chunks: u64, utf8_bytes: u64 },
}
pub(super) struct TextCommand {
    writer: Arc<TextWriter>,
    operation: TextOperation,
    reply: Option<oneshot::Sender<TextProgress>>,
    completed: bool,
}
impl TextCommand {
    pub(super) fn identity(&self) -> TextIdentity {
        self.writer.snapshot().identity
    }
    pub(super) fn run(
        mut self,
        out: &mut impl Write,
        line: &str,
        busy: &mut bool,
    ) -> io::Result<()> {
        let result = (|| {
            if self.writer.cancelled() {
                return Ok(());
            }
            match &self.operation {
                TextOperation::Start => {
                    execute!(out, MoveToColumn(0), Clear(ClearType::CurrentLine))?;
                    print_text(out, "alia: ")?;
                    *busy = true;
                }
                TextOperation::Chunk(chunk) => write_source(out, &chunk.text, &self.writer)?,
                TextOperation::End { .. } => {
                    print_text(out, "\n")?;
                    *busy = false;
                    render_prompt(out, line)?;
                }
            }
            Ok(())
        })();
        let progress = self.writer.finish(&self.operation, result.as_ref().err());
        self.completed = true;
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(progress);
        }
        result
    }
}
impl Drop for TextCommand {
    fn drop(&mut self) {
        if !self.completed {
            // Receiver closure discards only commands that the worker has not
            // entered. A native call retains this guard until that call returns.
            self.writer
                .seal("console command owner closed before acknowledgement");
            let progress = self.writer.finish(&self.operation, None);
            if let Some(reply) = self.reply.take() {
                let _ = reply.send(progress);
            }
        }
    }
}
fn write_source(out: &mut impl Write, source: &str, writer: &TextWriter) -> io::Result<()> {
    // Raw mode requires CR before LF. The inserted CR is console formatting and
    // never contributes to source evidence. All other bytes retain exact order.
    for segment in source.as_bytes().split_inclusive(|byte| *byte == b'\n') {
        let content = segment.strip_suffix(b"\n").unwrap_or(segment);
        let mut remaining = content;
        while !remaining.is_empty() && !writer.cancelled() {
            match out.write(remaining) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "stdout accepted zero bytes",
                    ));
                }
                Ok(n) => {
                    writer.written(n);
                    remaining = &remaining[n..];
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        if writer.cancelled() {
            break;
        }
        if segment.ends_with(b"\n") {
            out.write_all(b"\r")?;
            if writer.cancelled() {
                break;
            }
            out.write_all(b"\n")?;
            writer.written(1);
        }
    }
    out.flush()
}

impl CliChannel {
    async fn text_command(
        &self,
        writer: &Arc<TextWriter>,
        operation: TextOperation,
    ) -> anyhow::Result<TextProgress> {
        let sender = self
            .output
            .read()
            .await
            .clone()
            .context("CLI is not running")?;
        let permit = sender
            .reserve()
            .await
            .context("CLI output worker has stopped")?;
        writer.admit(&operation)?;
        let (reply, receive) = oneshot::channel();
        permit.send(ConsoleOutput::Text(TextCommand {
            writer: writer.clone(),
            operation,
            reply: Some(reply),
            completed: false,
        }));
        receive
            .await
            .context("CLI native writer stopped without checkpoint")
    }
    pub(super) async fn deliver_owned_text(
        &self,
        identity: TextIdentity,
        mut input: crate::modules::delivery::transport::TextPayloadStream,
        observer: Arc<dyn crate::modules::delivery::transport::TextObserver>,
        cancel: CancellationToken,
    ) -> anyhow::Result<crate::modules::delivery::transport::TextDeliveryReport> {
        ensure!(
            identity.channel_instance_id == self.instance_id,
            "text belongs to another CLI instance"
        );
        observer.start().await?;
        let writer = self.text_writers.open(identity)?;
        if cancel.is_cancelled() {
            writer.seal("response cancelled before writer Start");
        }
        let result = async {
            let progress = self.text_command(&writer, TextOperation::Start).await?;
            observer.progress(progress).await?;
            loop {
                let frame = tokio::select! { biased;
                    _ = cancel.cancelled() => { writer.seal("response cancelled"); break; },
                    frame = input.next() => frame,
                };
                match frame {
                    Some(Ok(TextFrame::Chunk(chunk))) => {
                        observer.offered(&chunk).await?;
                        let progress = self
                            .text_command(&writer, TextOperation::Chunk(chunk))
                            .await?;
                        observer.progress(progress.clone()).await?;
                        if progress.observation_end.is_some() {
                            break;
                        }
                    }
                    Some(Ok(TextFrame::End { chunks, utf8_bytes })) => {
                        observer.end(chunks, utf8_bytes).await?;
                        let progress = self
                            .text_command(&writer, TextOperation::End { chunks, utf8_bytes })
                            .await?;
                        observer.progress(progress).await?;
                        break;
                    }
                    Some(Err(error)) => return Err(error),
                    None => anyhow::bail!("text producer ended without explicit End"),
                }
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if let Err(error) = &result {
            writer.seal(&format!("text producer or observer stopped: {error}"));
        }
        let progress = writer.quiescent().await;
        if let Err(error) = result {
            return Err(error);
        }
        let observation_end = progress
            .observation_end
            .clone()
            .context("CLI text ended without terminal checkpoint")?;
        Ok(crate::modules::delivery::transport::TextDeliveryReport {
            progress: Some(progress),
            observation_end,
        })
    }
}

use futures::StreamExt;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::delivery::transport::{DeliveryTransport, TextObserver, TextPayloadStream};
    use alia_core::text_delivery::TextWriteBoundary;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Observer {
        offered: AtomicUsize,
        reject: bool,
    }
    #[async_trait]
    impl TextObserver for Observer {
        async fn start(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn offered(&self, chunk: &TextChunk) -> anyhow::Result<()> {
            ensure!(!self.reject, "injected durable offer failure");
            self.offered.fetch_add(chunk.text.len(), Ordering::SeqCst);
            Ok(())
        }
        async fn end(&self, _: u64, bytes: u64) -> anyhow::Result<()> {
            ensure!(
                bytes == self.offered.load(Ordering::SeqCst) as u64,
                "End differs from durable source bytes"
            );
            Ok(())
        }
        async fn progress(&self, p: TextProgress) -> anyhow::Result<()> {
            ensure!(
                p.accepted_utf8_bytes <= self.offered.load(Ordering::SeqCst) as u64,
                "write preceded durable offer"
            );
            Ok(())
        }
    }
    struct NativeWriter {
        output: Arc<StdMutex<Vec<u8>>>,
        entered: Option<std::sync::mpsc::Sender<()>>,
        release: Option<std::sync::mpsc::Receiver<()>>,
        fail_after_prefix: bool,
        prefix_written: bool,
    }
    impl Write for NativeWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.prefix_written && self.fail_after_prefix {
                return Err(io::Error::other("injected native write failure"));
            }
            if !self.prefix_written && bytes.first() == Some(&0xe4) {
                if let Some(entered) = self.entered.take() {
                    entered.send(()).unwrap();
                }
                if let Some(release) = self.release.take() {
                    release.recv().unwrap();
                }
                self.prefix_written = true;
                self.output.lock().unwrap().push(bytes[0]);
                return Ok(1);
            }
            self.output.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    async fn channel(writer: NativeWriter) -> Arc<CliChannel> {
        let channel = Arc::new(CliChannel::new("cli-native-test", EventBus::new(4)));
        let (sender, mut receiver) = mpsc::channel(2);
        *channel.output.write().await = Some(sender);
        *channel.worker.lock().await = Some(ConsoleWorker {
            cancel: CancellationToken::new(),
            handle: tokio::task::spawn_blocking(move || {
                let mut writer = writer;
                let mut busy = false;
                while let Some(command) = receiver.blocking_recv() {
                    match command {
                        ConsoleOutput::Text(command) => command.run(&mut writer, "", &mut busy)?,
                        _ => unreachable!("test only admits owned text"),
                    }
                }
                Ok(())
            }),
        });
        channel
    }
    fn input(end: bool) -> TextPayloadStream {
        let mut frames = vec![Ok(TextFrame::Chunk(TextChunk {
            sequence: 0,
            utf8_offset: 0,
            text: "你好".into(),
        }))];
        if end {
            frames.push(Ok(TextFrame::End {
                chunks: 1,
                utf8_bytes: 6,
            }));
        }
        Box::pin(futures::stream::iter(frames))
    }
    fn id(channel: &CliChannel) -> TextIdentity {
        TextIdentity {
            text_id: uuid::Uuid::new_v4(),
            channel_instance_id: channel.instance_id,
        }
    }
    fn target() -> DeliveryTarget {
        DeliveryTarget {
            voice: None,
            user_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    #[tokio::test]
    async fn exact_cancel_waits_for_native_write_and_survives_dropped_delivery_waiter() {
        let output = Arc::new(StdMutex::new(Vec::new()));
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let channel = channel(NativeWriter {
            output: output.clone(),
            entered: Some(entered),
            release: Some(blocked),
            fail_after_prefix: false,
            prefix_written: false,
        })
        .await;
        let identity = id(&channel);
        let delivery_channel = channel.clone();
        let delivery = tokio::spawn(async move {
            delivery_channel
                .deliver_text(
                    &target(),
                    identity,
                    input(true),
                    Arc::new(Observer::default()),
                    CancellationToken::new(),
                )
                .await
        });
        tokio::task::spawn_blocking(move || observed.recv_timeout(Duration::from_secs(3)).unwrap())
            .await
            .unwrap();
        delivery.abort();
        assert!(delivery.await.unwrap_err().is_cancelled());
        assert!(
            tokio::time::timeout(
                Duration::from_millis(30),
                channel.cancel_text(identity, "cancel after waiter drop")
            )
            .await
            .is_err()
        );
        assert!(!channel.text_writers.is_terminal(identity));
        release.send(()).unwrap();
        let checkpoint = tokio::time::timeout(
            Duration::from_secs(2),
            channel.cancel_text(identity, "cancel after waiter drop"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(checkpoint.accepted_utf8_bytes, 6);
        assert_eq!(checkpoint.written_utf8_bytes, Some(1));
        assert_eq!(
            checkpoint.write_boundary,
            Some(TextWriteBoundary::StdoutWrite)
        );
        assert!(matches!(
            checkpoint.observation_end,
            Some(ObservationEnd::Cancelled { .. })
        ));
        assert!(!checkpoint.end_accepted);
        assert!(checkpoint.validate(None, 6, false).is_ok());
        assert_eq!(output.lock().unwrap().last(), Some(&0xe4));
        channel.stop().await.unwrap();
    }

    #[tokio::test]
    async fn full_text_requires_end_and_stdout_failure_keeps_partial_utf8_fact() {
        for (end, fail) in [(true, false), (false, false), (true, true)] {
            let channel = channel(NativeWriter {
                output: Default::default(),
                entered: None,
                release: None,
                fail_after_prefix: fail,
                prefix_written: false,
            })
            .await;
            let identity = id(&channel);
            let result = channel
                .deliver_text(
                    &target(),
                    identity,
                    input(end),
                    Arc::new(Observer::default()),
                    CancellationToken::new(),
                )
                .await;
            let final_progress = channel
                .cancel_text(identity, "inspect original writer")
                .await
                .unwrap();
            if fail {
                assert!(matches!(
                    result.unwrap().observation_end,
                    ObservationEnd::Failed { .. }
                ));
                assert_eq!(final_progress.written_utf8_bytes, Some(1));
                assert!(channel.stop().await.is_err());
            } else if end {
                assert_eq!(
                    result.unwrap().observation_end,
                    ObservationEnd::SubmissionFinished
                );
                assert_eq!(final_progress.written_utf8_bytes, Some(6));
                assert!(final_progress.end_accepted);
                channel.stop().await.unwrap();
            } else {
                assert!(result.is_err());
                assert!(!final_progress.end_accepted);
                assert_ne!(
                    final_progress.observation_end,
                    Some(ObservationEnd::SubmissionFinished)
                );
                channel.stop().await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn failed_durable_offer_and_foreign_identity_never_reach_native_source_write() {
        let output = Arc::new(StdMutex::new(Vec::new()));
        let channel = channel(NativeWriter {
            output: output.clone(),
            entered: None,
            release: None,
            fail_after_prefix: false,
            prefix_written: false,
        })
        .await;
        let identity = id(&channel);
        assert!(
            channel
                .deliver_text(
                    &target(),
                    identity,
                    input(true),
                    Arc::new(Observer {
                        reject: true,
                        ..Default::default()
                    }),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        assert_eq!(
            channel
                .cancel_text(identity, "failed observer")
                .await
                .unwrap()
                .written_utf8_bytes,
            Some(0)
        );
        assert!(!output.lock().unwrap().contains(&0xe4));
        let wrong = TextIdentity {
            channel_instance_id: uuid::Uuid::new_v4(),
            ..identity
        };
        assert!(
            channel
                .cancel_text(wrong, "foreign instance")
                .await
                .is_err()
        );
        assert!(
            channel
                .deliver_text(
                    &target(),
                    wrong,
                    input(true),
                    Arc::new(Observer::default()),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        channel.stop().await.unwrap();
    }

    #[tokio::test]
    async fn old_exact_cancel_cannot_close_a_new_text_writer() {
        let book = TextWriters::default();
        let instance = uuid::Uuid::new_v4();
        let first = TextIdentity {
            text_id: uuid::Uuid::new_v4(),
            channel_instance_id: instance,
        };
        let second = TextIdentity {
            text_id: uuid::Uuid::new_v4(),
            channel_instance_id: instance,
        };
        let old = book.open(first).unwrap();
        assert!(book.open(second).is_err());
        old.seal("first response cancelled");
        let current = book.open(second).unwrap();
        let original = book
            .cancel(first, "late cancel for old response")
            .await
            .unwrap();
        assert_eq!(original.identity, first);
        assert!(!current.cancelled());
        assert!(current.snapshot().observation_end.is_none());
        assert!(book.open(first).is_err());
        book.stop();
    }

    #[tokio::test]
    async fn model_terminal_control_sequences_are_rejected_before_native_write() {
        let output = Arc::new(StdMutex::new(Vec::new()));
        let channel = channel(NativeWriter {
            output: output.clone(),
            entered: None,
            release: None,
            fail_after_prefix: false,
            prefix_written: false,
        })
        .await;
        let identity = id(&channel);
        let control = "\u{1b}]52;c;dW50cnVzdGVk\u{7}";
        let frames = Box::pin(futures::stream::iter([Ok(TextFrame::Chunk(TextChunk {
            sequence: 0,
            utf8_offset: 0,
            text: control.into(),
        }))]));
        assert!(
            channel
                .deliver_text(
                    &target(),
                    identity,
                    frames,
                    Arc::new(Observer::default()),
                    CancellationToken::new()
                )
                .await
                .is_err()
        );
        let checkpoint = channel
            .cancel_text(identity, "control data rejected")
            .await
            .unwrap();
        assert_eq!(checkpoint.accepted_utf8_bytes, 0);
        assert_eq!(checkpoint.written_utf8_bytes, Some(0));
        assert!(
            !output
                .lock()
                .unwrap()
                .windows(4)
                .any(|bytes| bytes == b"]52;")
        );
        channel.stop().await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a real PTY; run with --ignored --nocapture --test-threads=1"]
    async fn real_console_write_proves_utf8_bytes_and_restores_terminal() {
        assert!(io::stdin().is_terminal() && io::stdout().is_terminal());
        let channel = CliChannel::new("cli-real-text", EventBus::new(4));
        channel.start().await.unwrap();
        let identity = id(&channel);
        let result = channel
            .deliver_text(
                &target(),
                identity,
                input(true),
                Arc::new(Observer::default()),
                CancellationToken::new(),
            )
            .await;
        channel.stop().await.unwrap();
        assert!(!terminal::is_raw_mode_enabled().unwrap());
        let report = result.unwrap();
        let progress = report.progress.unwrap();
        assert_eq!(report.observation_end, ObservationEnd::SubmissionFinished);
        assert_eq!(progress.written_utf8_bytes, Some(6));
        assert_eq!(
            progress.write_boundary,
            Some(TextWriteBoundary::StdoutWrite)
        );
        assert!(progress.end_accepted);
    }
}
