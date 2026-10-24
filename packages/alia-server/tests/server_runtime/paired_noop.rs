//! Real loopback HTTP + signed gRPC binding, with all hardware/model slots disabled.
use super::*;
use alia_embedded::binding::BindingState;
use alia_embedded::config::{EmbeddedConfig, ModulesConfig};
use alia_embedded::module::EmbeddedBuilder;
use alia_server::config::ChannelConfig;
use base64::Engine;
use serde_json::{Value, json};

#[tokio::test]
async fn paired_noop_runtimes_restore_binding_and_release_resources_after_restart() {
    with_fixture(|mut f| async move {
        let mut embedded = EmbeddedConfig::default();
        embedded.config_path = f.dir.join("embedded.toml");
        embedded.identity.device_id = format!("noop-{}", Uuid::new_v4());
        embedded.identity.keypair_path = f.dir.join("device.key");
        embedded.device_runtime.listen_addr = "127.0.0.1:0".parse().unwrap();
        embedded.device_runtime.state_path = f.dir.join("embedded.sqlite");
        embedded.device_runtime.plugins_dir = f.dir.join("plugins");
        embedded.device_runtime.shutdown_grace_ms = 3_000;
        embedded.device_runtime.grpc_drain_timeout_ms = 500;
        embedded.modules = ModulesConfig {
            wakeword: "noop".into(),
            vad: "noop".into(),
            direct_response: "noop".into(),
            audio_capture: "noop".into(),
            audio_playback: "noop".into(),
            face_tracker: "noop".into(),
            discovery: "noop".into(),
        };
        embedded.motion.enabled = false;
        embedded.vision.enabled = false;
        embedded.vision_debug.enabled = false;
        embedded.hmmd_mmwave.enabled = false;
        assert!(!embedded.dev.skip_binding);
        alia_embedded::identity::ensure_device_keypair(&embedded.identity.keypair_path).unwrap();
        alia_embedded::config::save_runtime_config(&embedded).unwrap();
        let device_id = embedded.identity.device_id.clone();
        let device_key =
            alia_embedded::identity::load_device_public_key(&embedded.identity.keypair_path)
                .unwrap();
        let mut bound_since = None;
        let mut server_public_key = None;
        let setup = f.builder().build().await.unwrap();
        let (owner, admin_token) = delivery::account(setup.state(), "admin").await;
        setup.shutdown().await.unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        for restart in [false, true] {
            let config = alia_embedded::config::load_config(&embedded.config_path).unwrap();
            assert_eq!(config.identity.device_id, device_id);
            let node = EmbeddedBuilder::from_config(config)
                .await
                .unwrap()
                .build()
                .await
                .unwrap();
            let binding = node.binding_manager.clone();
            let before = binding.status().await;
            assert_eq!(
                before.state,
                if restart {
                    BindingState::Bound
                } else {
                    BindingState::Unbound
                }
            );
            assert!(!before.dev_skip_auth);
            let embedded_addr = node.local_addr();
            let embedded_stop = node.cancel_token.clone();
            let _embedded_drop = embedded_stop.clone().drop_guard();
            let embedded_task = tokio::spawn(node.run());

            f.config.server.grpc_port = Some(0);
            f.config.channels = vec![ChannelConfig::Embedded {
                id: "paired-noop".into(),
                owner_user_id: Some(owner),
                expected_device_id: Some(device_id.clone()),
                expected_device_public_key: Some(
                    base64::engine::general_purpose::STANDARD.encode(&device_key),
                ),
                approved_animations: vec![],
                enabled: true,
                endpoint: format!("http://{embedded_addr}"),
                discovery_service_name: "unused-noop".into(),
                discovery_timeout_sec: 1,
                reconnect_interval_sec: 1,
                reconnect_max_backoff_sec: 1,
                grpc_timeout_sec: 3,
            }];
            let runtime = f.builder().build().await.unwrap();
            let state = runtime.state().clone();
            let token = &admin_token;
            let server_addr = runtime.http_addr();
            let grpc_addr = runtime.grpc_addr().unwrap();
            let server_stop = state.cancel_token.clone();
            let _server_drop = server_stop.clone().drop_guard();
            let server_task = tokio::spawn(runtime.run());
            let base = format!("http://{server_addr}/api/v1/device");
            if !restart {
                let response = client
                    .post(format!("{base}/bind"))
                    .bearer_auth(token)
                    .json(
                        &json!({"channel_id":"paired-noop", "require_confirmation":false,
                        "server_endpoint":format!("http://{grpc_addr}")}),
                    )
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
                assert_eq!(response.json::<Value>().await.unwrap()["status"], "bound");
            }
            let persisted = binding.status().await;
            assert_eq!(persisted.state, BindingState::Bound);
            assert_eq!(
                persisted.server_id.as_deref(),
                Some(f.config.server.id.as_str())
            );
            assert!(!persisted.dev_skip_auth);
            if restart {
                assert_eq!(persisted.bound_since, bound_since);
                assert_eq!(persisted.server_public_key, server_public_key);
            } else {
                bound_since = persisted.bound_since;
                server_public_key = persisted.server_public_key;
                assert_eq!(server_public_key.as_ref().unwrap().len(), 32);
            }
            let status = client
                .get(format!("{base}/status?channel_id=paired-noop"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            assert_eq!(status.status(), 200);
            let status: Value = status.json().await.unwrap();
            assert_eq!(status["device_id"], device_id);
            assert_eq!(status["online"], true);
            // Settings requires a fresh bearer signed by the original persisted server key.
            let settings = client
                .get(format!("{base}/settings?channel_id=paired-noop"))
                .bearer_auth(token)
                .send()
                .await
                .unwrap();
            assert_eq!(settings.status(), 200);
            assert!(settings.json::<Value>().await.unwrap().is_object());
            drop(state);
            server_stop.cancel();
            tokio::time::timeout(Duration::from_secs(5), server_task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            embedded_stop.cancel();
            tokio::time::timeout(Duration::from_secs(5), embedded_task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            drop(binding);
            for address in [server_addr, grpc_addr, embedded_addr] {
                let _released = tokio::net::TcpListener::bind(address).await.unwrap();
            }
            assert_eq!(
                alia_embedded::identity::load_device_public_key(&embedded.identity.keypair_path)
                    .unwrap(),
                device_key
            );
        }
    })
    .await;
}
