use super::*;

fn channel() -> EmbeddedChannel {
    EmbeddedChannel::new(
        EmbeddedChannelConfig {
            id: "sensor-cancellation-test".into(),
            owner_user_id: None,
            expected_device_id: None,
            expected_device_public_key: None,
            approved_animations: vec![],
            endpoint: String::new(),
            discovery_service_name: "_alia-test._tcp".into(),
            discovery_timeout_sec: 1,
            reconnect_interval_sec: 1,
            reconnect_max_backoff_sec: 1,
            grpc_timeout_sec: 1,
        },
        EventBus::new(2),
    )
}

#[tokio::test]
async fn plugin_command_rejects_non_string_payload_before_connecting() {
    let channel = channel();
    // A valid action reaches the stopped-client guard; no network is started.
    channel.discovery_cancel.cancel();
    let mut params = serde_json::json!({
        "plugin": "echo",
        "instance_id": "original-instance",
        "command_id": "original-command",
        "action": "echo",
    });
    for payload in [
        serde_json::json!({"value": 1}),
        serde_json::json!([1]),
        serde_json::json!(1),
        serde_json::json!(true),
        serde_json::Value::Null,
    ] {
        params["payload"] = payload;
        let error = channel
            .execute_action(ChannelAction {
                name: "send_plugin_command".into(),
                params: params.clone(),
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "send_plugin_command requires string 'payload' when provided"
        );
    }
    for payload in [None, Some(serde_json::json!(r#"{"value":1}"#))] {
        params.as_object_mut().unwrap().remove("payload");
        if let Some(payload) = payload {
            params["payload"] = payload;
        }
        let error = channel
            .execute_action(ChannelAction {
                name: "send_plugin_command".into(),
                params: params.clone(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "embedded channel is stopping");
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn sensor_sync_waits_for_background_owner_cancellation() {
    let channel = channel();
    let background = OwnedBackground::default();
    let parent = CancellationToken::new();
    let mut startup = background.begin_start(&parent).await.unwrap();
    let registry_owners = Arc::strong_count(&channel.sensor_registry);
    let client_owners = Arc::strong_count(&channel.sensor_client);

    // Model the interval after discovery cancellation but before the owner's
    // parent bridge runs. No discovery task, RPC, or hardware is started here.
    channel.discovery_cancel.cancel();
    channel.spawn_sensor_sync_loop(&mut startup);
    startup.commit();

    // Paused current-thread time advances only once runnable work is idle.
    // The first interval tick may run, but the empty registry cannot await IO;
    // the next tick is 30s away. This is not a wall-clock scheduling guess.
    tokio::time::sleep(Duration::from_millis(1)).await;
    let owner_active = !background.is_stopping();
    let sensor_job_retained = Arc::strong_count(&channel.sensor_registry) == registry_owners + 1
        && Arc::strong_count(&channel.sensor_client) == client_owners + 1;

    // Join the original handles before any observation can fail the case.
    let stopped = background.stop().await;
    let stopped_again = background.stop().await;
    let resources_released = Arc::strong_count(&channel.sensor_registry) == registry_owners
        && Arc::strong_count(&channel.sensor_client) == client_owners;
    eprintln!(
        "sensor cancellation: owner_active={owner_active} sensor_job_retained={sensor_job_retained} resources_released={resources_released} stopped={stopped:?} stopped_again={stopped_again:?}"
    );

    assert!(owner_active, "the background owner was not cancelled");
    assert!(resources_released, "the original sensor job must be joined");
    assert!(
        stopped.is_ok(),
        "discovery cancellation must not produce a false early exit: {stopped:?}"
    );
    assert!(stopped_again.is_ok());
    assert!(sensor_job_retained, "the owner must retain the sensor loop");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn actual_loop_early_return_stays_a_shutdown_failure() {
    let background = OwnedBackground::default();
    let parent = CancellationToken::new();
    let mut startup = background.begin_start(&parent).await.unwrap();
    startup.spawn_loop("unexpected successful exit", async {});
    startup.commit();

    // Drain the immediately runnable job before requesting owner cancellation.
    tokio::time::sleep(Duration::from_millis(1)).await;
    let owner_active = !background.is_stopping();
    let stopped = background.stop().await;
    let stopped_again = background.stop().await;

    assert!(owner_active);
    let error = stopped.unwrap_err().to_string();
    assert!(error.contains("unexpected successful exit"), "{error}");
    assert!(
        error.contains("background task exited before cancellation"),
        "{error}"
    );
    assert_eq!(stopped_again.unwrap_err().to_string(), error);
}
