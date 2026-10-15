use super::delivery_support::run_context;
use futures::FutureExt;
use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Context {
    events: Arc<Mutex<Vec<&'static str>>>,
    initialized: bool,
}

struct BodyGuard(Arc<Mutex<Vec<&'static str>>>);
impl Drop for BodyGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("body dropped");
    }
}

#[tokio::test(start_paused = true)]
async fn outer_deadline_drops_body_then_cleans_context_before_database() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let database_events = events.clone();
    let started = tokio::time::Instant::now();
    let result = AssertUnwindSafe(run_context(
        Context {
            events: events.clone(),
            initialized: false,
        },
        |context| {
            Box::pin(async move {
                context.initialized = true;
                let _body = BodyGuard(context.events.clone());
                std::future::pending::<()>().await;
            })
        },
        |context| async move {
            assert!(context.initialized);
            assert_eq!(*context.events.lock().unwrap(), ["body dropped"]);
            context.events.lock().unwrap().push("context cleaned");
        },
        move || async move {
            assert_eq!(
                *database_events.lock().unwrap(),
                ["body dropped", "context cleaned"]
            );
            database_events.lock().unwrap().push("database cleaned");
        },
    ))
    .catch_unwind()
    .await;
    let error = result.unwrap_err();
    assert_eq!(
        error.downcast_ref::<&str>(),
        Some(&"Delivery test exceeded 45s")
    );
    assert_eq!(started.elapsed(), Duration::from_secs(45));
    assert_eq!(
        *events.lock().unwrap(),
        ["body dropped", "context cleaned", "database cleaned"]
    );
}

#[derive(Debug, PartialEq, Eq)]
struct OriginalFailure;

#[tokio::test(start_paused = true)]
async fn cleanup_crossing_original_deadline_fails_after_draining_once() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let database_events = events.clone();
    let started = tokio::time::Instant::now();
    let result = AssertUnwindSafe(run_context(
        events.clone(),
        |events| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(44)).await;
                events.lock().unwrap().push("body completed");
            })
        },
        |events| async move {
            events.lock().unwrap().push("cleanup started");
            tokio::time::sleep(Duration::from_secs(2)).await;
            events.lock().unwrap().push("cleanup completed");
        },
        move || async move {
            database_events.lock().unwrap().push("database cleaned");
        },
    ))
    .catch_unwind()
    .await;
    assert_eq!(
        result.unwrap_err().downcast_ref::<&str>(),
        Some(&"Delivery test exceeded 45s")
    );
    assert_eq!(started.elapsed(), Duration::from_secs(46));
    assert_eq!(
        *events.lock().unwrap(),
        [
            "body completed",
            "cleanup started",
            "cleanup completed",
            "database cleaned"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn primary_panic_survives_cleanup_crossing_original_deadline() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let database_events = events.clone();
    let started = tokio::time::Instant::now();
    let result = AssertUnwindSafe(run_context(
        events.clone(),
        |_| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(44)).await;
                std::panic::panic_any(OriginalFailure);
            })
        },
        |events| async move {
            events.lock().unwrap().push("cleanup started");
            tokio::time::sleep(Duration::from_secs(2)).await;
            events.lock().unwrap().push("cleanup completed");
        },
        move || async move {
            database_events.lock().unwrap().push("database cleaned");
        },
    ))
    .catch_unwind()
    .await;
    assert_eq!(
        result.unwrap_err().downcast_ref::<OriginalFailure>(),
        Some(&OriginalFailure)
    );
    assert_eq!(started.elapsed(), Duration::from_secs(46));
    assert_eq!(
        *events.lock().unwrap(),
        ["cleanup started", "cleanup completed", "database cleaned"]
    );
}

#[tokio::test]
async fn primary_panic_survives_both_cleanup_failures() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let database_events = events.clone();
    let result = AssertUnwindSafe(run_context(
        events.clone(),
        |_| Box::pin(async { std::panic::panic_any(OriginalFailure) }),
        |events| async move {
            events.lock().unwrap().push("context attempted");
            panic!("secondary context failure");
        },
        move || async move {
            database_events.lock().unwrap().push("database attempted");
            panic!("secondary database failure");
        },
    ))
    .catch_unwind()
    .await;
    assert_eq!(
        result.unwrap_err().downcast_ref::<OriginalFailure>(),
        Some(&OriginalFailure)
    );
    assert_eq!(
        *events.lock().unwrap(),
        ["context attempted", "database attempted"]
    );
}

#[tokio::test]
async fn cleanup_panic_still_attempts_database_cleanup_and_fails_case() {
    let cleaned = Arc::new(Mutex::new(false));
    let database_cleaned = cleaned.clone();
    let result = AssertUnwindSafe(run_context(
        (),
        |_| Box::pin(async {}),
        |_| async { panic!("context failure") },
        move || async move { *database_cleaned.lock().unwrap() = true },
    ))
    .catch_unwind()
    .await;
    assert_eq!(
        result.unwrap_err().downcast_ref::<&str>(),
        Some(&"context failure")
    );
    assert!(*cleaned.lock().unwrap());
}

#[tokio::test]
async fn database_cleanup_panic_fails_otherwise_successful_case() {
    let result = AssertUnwindSafe(run_context(
        (),
        |_| Box::pin(async {}),
        |_| async {},
        || async { panic!("database failure") },
    ))
    .catch_unwind()
    .await;
    assert_eq!(
        result.unwrap_err().downcast_ref::<&str>(),
        Some(&"database failure")
    );
}
