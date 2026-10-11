use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct ObservedLlm {
    initialized: Arc<AtomicUsize>,
    stopped: Arc<AtomicUsize>,
    fail_init: bool,
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

#[async_trait::async_trait]
impl LlmAdapter for ObservedLlm {
    async fn chat(&self, _: &[crate::providers::adapter::ChatMessage]) -> anyhow::Result<String> {
        unreachable!("profile ownership test does not call a model")
    }
    async fn chat_stream(
        &self,
        _: &[crate::providers::adapter::ChatMessage],
    ) -> anyhow::Result<std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<String>> + Send>>>
    {
        unreachable!("profile ownership test does not call a model")
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
            Box::new(ObservedLlm {
                initialized: initialized.clone(),
                stopped: stopped.clone(),
                fail_init,
            }) as Box<dyn LlmAdapter>
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
