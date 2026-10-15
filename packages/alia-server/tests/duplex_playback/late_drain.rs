//! Exercise the real paired supervisor's original shutdown budget with a held write.
use super::*;
use alia_embedded::module::playback_session::SubmissionWrite;

#[derive(Default)]
struct LateDrain {
    runtime: Option<Runtime>,
    producer: Option<Arc<OutputProducer>>,
    write: Option<SubmissionWrite>,
    dispatch: Option<tokio::task::JoinHandle<DeliveryResult<DeliveryRecord>>>,
}

struct DrainEvidence {
    scene: Scene,
    directory: PathBuf,
    listener: tokio::task::AbortHandle,
    observation_elapsed: Duration,
    close_pending: bool,
    supervisor_pending: bool,
    database_open_while_pending: bool,
    directory_retained: bool,
    listener_retained: bool,
    database_open_after_drain: bool,
    dispatch_joined: bool,
}

#[tokio::test]
async fn paired_late_drain_joins_original_write_before_database_cleanup_and_keeps_failure() {
    let evidence = Arc::new(Mutex::new(None));
    let completed = evidence.clone();
    let result = AssertUnwindSafe(with_db_context(
        LateDrain::default(),
        |f, owner| {
            Box::pin(async move {
                Runtime::initialize(&mut owner.runtime, f, 4096).await;
                let scene = owner.runtime.as_ref().unwrap().scene.clone();
                let (producer, response) = scene.output().await;
                owner.producer = Some(producer);
                let input = scene.audio(&response, &[17]).await;
                let service = scene.service.clone();
                owner.dispatch = Some(tokio::spawn(
                    async move { service.deliver_audio(input).await },
                ));
                media_signal(&scene.device.ended_event, owner.dispatch.as_mut().unwrap()).await;
                eventually(|| async {
                    scene
                        .submission()
                        .is_some_and(|value| value.snapshot(0).end_accepted)
                })
                .await;
                owner.write = Some(scene.submission().unwrap().try_begin_output().unwrap());
            })
        },
        move |mut owner| async move {
            let Some(runtime) = owner.runtime.take() else {
                return;
            };
            let scene = runtime.scene.clone();
            let directory = runtime._directory.0.clone();
            let listener = runtime.server.task.as_ref().unwrap().abort_handle();
            let mut close = Box::pin(AssertUnwindSafe(runtime.close()).catch_unwind());
            let held_write = owner.write.is_some();
            let started = tokio::time::Instant::now();
            let observed = if held_write {
                // Observe beyond the unchanged ten-second shutdown budget in real time.
                tokio::time::timeout(Duration::from_secs(11), close.as_mut()).await
            } else {
                Ok(close.as_mut().await)
            };
            let observation_elapsed = started.elapsed();
            let close_pending = observed.is_err();
            let supervisor_pending = scene.service.shutdown_pending();
            let directory_retained = directory.exists();
            let listener_retained = !listener.is_finished();
            let database_open_while_pending = matches!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    scene.f.db.execute_unprepared("SELECT 1")
                )
                .await,
                Ok(Ok(_))
            );
            // Record observations first; every assertion follows release and real joins.
            if let Some(write) = owner.write.take() {
                write.finish(1);
            }
            let closed = match observed {
                Ok(result) => result,
                Err(_) => close.as_mut().await,
            };
            drop(close);
            let dispatch_joined = match owner.dispatch.take() {
                Some(task) => matches!(task.await, Ok(Ok(_))),
                None => false,
            };
            let database_open_after_drain = scene.f.db.execute_unprepared("SELECT 1").await.is_ok();
            if held_write {
                *completed.lock().unwrap() = Some(DrainEvidence {
                    scene,
                    directory,
                    listener,
                    observation_elapsed,
                    close_pending,
                    supervisor_pending,
                    database_open_while_pending,
                    directory_retained,
                    listener_retained,
                    database_open_after_drain,
                    dispatch_joined,
                });
            }
            if let Err(error) = closed {
                std::panic::resume_unwind(error);
            }
        },
    ))
    .catch_unwind()
    .await;

    // These checks are outside the expected failure path, so no earlier panic masks them.
    let evidence = evidence
        .lock()
        .unwrap()
        .take()
        .expect("late drain was not observed");
    assert!(evidence.observation_elapsed >= Duration::from_secs(10));
    assert!(evidence.close_pending);
    assert!(evidence.supervisor_pending);
    assert!(evidence.database_open_while_pending);
    assert!(evidence.directory_retained);
    assert!(evidence.listener_retained);
    assert!(evidence.dispatch_joined);
    assert!(evidence.database_open_after_drain);
    assert!(!evidence.scene.service.shutdown_pending());
    assert!(!evidence.scene.session.is_active());
    assert!(evidence.scene.submission().unwrap().output_quiescent());
    assert!(evidence.listener.is_finished());
    assert!(!evidence.directory.exists());
    assert!(
        evidence
            .scene
            .f
            .db
            .execute_unprepared("SELECT 1")
            .await
            .is_err()
    );
    let error = result.expect_err("the first shutdown deadline must remain a failure");
    let text = error
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| error.downcast_ref::<String>().map(String::as_str));
    assert_eq!(text, Some("paired Delivery cleanup failed"));
}
