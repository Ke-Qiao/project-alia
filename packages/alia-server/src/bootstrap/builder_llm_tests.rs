use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct ObservedLlm {
    initialized: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
    fail_init: bool,
    response: &'static str,
}

#[async_trait::async_trait]
impl ModuleLifecycle for ObservedLlm {
    fn name(&self) -> &'static str {
        "shared-profile-test"
    }
    async fn init(&mut self) -> anyhow::Result<()> {
        self.initialized.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(!self.fail_init, "profile init failed");
        Ok(())
    }
    async fn stop(&self) -> anyhow::Result<()> {
        self.stopped.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl ObservedLlm {
    fn into_service(self) -> LlmService {
        let response = self.response;
        LlmService::test_text("shared-profile-test", move |_| async move {
            Ok(response.to_owned())
        })
        .with_lifecycle(Box::new(self))
    }
}

#[tokio::test]
async fn configured_roles_share_one_init_and_cleanup_owner_but_overrides_do_not() {
    for fail_custom in [false, true] {
        let cancel = CancellationToken::new();
        let cleanup = RuntimeCleanup::new(cancel.clone(), Duration::from_secs(1));
        let initialized = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicUsize::new(0));
        let module = |fail_init| {
            Box::new(
                ObservedLlm {
                    initialized: initialized.clone(),
                    stopped: stopped.clone(),
                    fail_init,
                    response: "shared",
                }
                .into_service(),
            )
        };
        let mut pending = HashMap::from([("shared".to_string(), module(false))]);
        let mut ready = HashMap::new();
        let first = initialize_llm_role(
            None,
            Some("shared"),
            &mut pending,
            &mut ready,
            &cleanup,
            &cancel,
        )
        .await
        .unwrap()
        .unwrap();
        let second = initialize_llm_role(
            None,
            Some("shared"),
            &mut pending,
            &mut ready,
            &cleanup,
            &cancel,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(initialized.load(Ordering::SeqCst), 1);
        assert!(pending.is_empty());
        assert!(
            initialize_llm_role(None, None, &mut pending, &mut ready, &cleanup, &cancel)
                .await
                .unwrap()
                .is_none()
        );

        let custom = initialize_llm_role(
            Some(module(fail_custom)),
            Some("shared"),
            &mut pending,
            &mut ready,
            &cleanup,
            &cancel,
        )
        .await;
        if fail_custom {
            assert!(custom.is_err());
        } else {
            assert!(!Arc::ptr_eq(&first, &custom.unwrap().unwrap()));
        }
        assert_eq!(initialized.load(Ordering::SeqCst), 2);
        cleanup
            .start()
            .wait_until(cleanup.deadline())
            .await
            .unwrap();
        assert_eq!(
            stopped.load(Ordering::SeqCst),
            2,
            "one shared owner plus the independent override, including failed init"
        );
    }
}

#[tokio::test]
async fn default_profile_selects_third_noop_and_alias_with_one_owner() {
    let cancel = CancellationToken::new();
    let cleanup = RuntimeCleanup::new(cancel.clone(), Duration::from_secs(1));
    let initialized = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(AtomicUsize::new(0));
    let module = |response| {
        Box::new(
            ObservedLlm {
                initialized: initialized.clone(),
                stopped: stopped.clone(),
                fail_init: false,
                response,
            }
            .into_service(),
        )
    };
    let mut pending = HashMap::from([
        ("minor".into(), module("minor")),
        ("major".into(), module("major")),
        ("third".into(), module("third")),
        ("noop".into(), Box::new(LlmService::noop())),
    ]);
    let mut ready = HashMap::new();
    let minor = initialize_llm_role(
        None,
        Some("minor"),
        &mut pending,
        &mut ready,
        &cleanup,
        &cancel,
    )
    .await
    .unwrap()
    .unwrap();
    let major = initialize_llm_role(
        None,
        Some("major"),
        &mut pending,
        &mut ready,
        &cleanup,
        &cancel,
    )
    .await
    .unwrap()
    .unwrap();
    for (profile, expected) in [
        ("third", "third"),
        ("noop", "LLM module not configured"),
        ("minor", "minor"),
        ("major", "major"),
        ("third", "third"),
    ] {
        let default = initialize_default_llm(
            profile,
            [("minor", &minor), ("major", &major)],
            &mut pending,
            &mut ready,
            &cleanup,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(default.text(&[]).await.unwrap(), expected);
        assert!(Arc::ptr_eq(&default, &ready[profile]));
    }
    // Routing can be disabled independently; default=major still selects its declared profile.
    let noop = ready["noop"].clone();
    let default = initialize_default_llm(
        "major",
        [("noop", &noop), ("noop", &noop)],
        &mut pending,
        &mut ready,
        &cleanup,
        &cancel,
    )
    .await
    .unwrap();
    assert!(Arc::ptr_eq(&default, &major));
    assert_eq!(initialized.load(Ordering::SeqCst), 3);
    cleanup
        .start()
        .wait_until(cleanup.deadline())
        .await
        .unwrap();
    assert_eq!(stopped.load(Ordering::SeqCst), 3);
}
