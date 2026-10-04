use super::*;
use crate::{
    binding::BindingState,
    config::{EmbeddedConfig, MotionConfig},
    grpc::DeviceBindingService,
    module::{
        lifecycle::ModuleLifecycle,
        noop::{NoopAudioPlayback, NoopDiscovery},
    },
    plugin::PluginRegistry,
    settings::EmbeddedSettingsManager,
    storage::EmbeddedStore,
};
use alia_core::delivery::VoiceScope;
use alia_proto::binding::{
    BindRequest, UnbindRequest, device_binding_client::DeviceBindingClient,
    device_binding_server::DeviceBindingServer,
};
use alia_proto::body::{
    body_control_client::BodyControlClient, body_control_server::BodyControlServer,
};
use alia_transport::{
    EMBEDDED_CONTROL_SCOPE, bind_payload, body_request_payload, current_unix_timestamp,
    mint_auth_token, verify_payload,
};
use futures::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe, pin::Pin};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Fixture {
    _dir: tempfile::TempDir,
    client: BodyControlClient<tonic::transport::Channel>,
    binding_client: DeviceBindingClient<tonic::transport::Channel>,
    endpoint: String,
    binding: Arc<BindingManager>,
    motion: Arc<MotionRuntime>,
    playback: Arc<PlaybackSession>,
    server_key: SigningKey,
    device_key: SigningKey,
    stop: CancellationToken,
    server: tokio::task::JoinHandle<std::result::Result<(), tonic::transport::Error>>,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let matrix = dir.path().join("neck.json");
        std::fs::write(&matrix, serde_json::json!({"ServoMinMax":{"servo_0":[0,180],"servo_1":[0,180],"servo_2":[0,180]},"NeckAngles":[[0,0,0]],"MatrixSize":[1,3],"sigma":100.0,"Matrix":[[90.0,90.0,90.0]],"NeckAngleMinMax":[-45,45,-45,45,-45,45]}).to_string()).unwrap();
        let config = MotionConfig {
            neck_config_file: matrix,
            servo_count: 3,
            hardware_enabled: false,
            soft_start_enabled: false,
            reset_on_stop: false,
            fps: 30,
            ..MotionConfig::default()
        };
        let motion = Arc::new(MotionRuntime::new_with_boot(config, Uuid::new_v4()).unwrap());
        motion.start().await.unwrap();
        let playback = Arc::new(PlaybackSession::with_output_fence(
            Arc::new(NoopAudioPlayback),
            motion.clone(),
        ));
        let binding = Arc::new(BindingManager::new(
            EmbeddedStore::open(dir.path().join("state.db"))
                .await
                .unwrap(),
            5,
            30,
        ));
        let server_key = SigningKey::from_bytes(&[19; 32]);
        let device_key = SigningKey::from_bytes(&[31; 32]);
        let issued = current_unix_timestamp().unwrap();
        let public = server_key.verifying_key().to_bytes();
        let signature = sign_payload(
            &server_key,
            &bind_payload(
                "body-test-server",
                "Test",
                "http://127.0.0.1:1",
                &public,
                issued,
            ),
        );
        binding
            .bind(
                "body-test-server",
                "Test",
                "http://127.0.0.1:1",
                &public,
                &signature,
                issued,
            )
            .await
            .unwrap();
        let revocation = motion.clone();
        binding
            .install_output_revocation(Arc::new(move || {
                revocation.revoke_control("binding changed").unwrap();
            }))
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let incoming = async_stream::stream! { loop { yield listener.accept().await.map(|(socket,_)|socket); }};
        let service = EmbeddedBodyService::new(
            binding.clone(),
            motion.clone(),
            playback.clone(),
            "body-test-device".into(),
            device_key.clone(),
        );
        let mut config = EmbeddedConfig::default();
        config.identity.device_id = "body-test-device".into();
        config.config_path = dir.path().join("embedded.toml");
        let binding_service = DeviceBindingService::new(
            binding.clone(),
            config.identity.clone(),
            Arc::new(NoopDiscovery),
            Arc::new(PluginRegistry::new(dir.path().join("plugins"))),
            motion.clone(),
            Arc::new(EmbeddedSettingsManager::new(&config)),
            device_key.verifying_key().to_bytes().to_vec(),
        );
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(BodyControlServer::new(service))
                .add_service(DeviceBindingServer::new(binding_service))
                .serve_with_incoming_shutdown(incoming, shutdown.cancelled())
                .await
        });
        let endpoint = format!("http://{address}");
        let transport = tonic::transport::Channel::from_shared(endpoint.clone())
            .unwrap()
            .connect()
            .await
            .unwrap();
        let client = BodyControlClient::new(transport.clone());
        let binding_client = DeviceBindingClient::new(transport);
        Self {
            _dir: dir,
            client,
            binding_client,
            endpoint,
            binding,
            motion,
            playback,
            server_key,
            device_key,
            stop,
            server,
        }
    }
    fn request(&self, operation: BodyOperation) -> BodyRequest {
        BodyRequest {
            version: 1,
            request_id: Uuid::new_v4(),
            device_id: "body-test-device".into(),
            issued_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            operation,
        }
    }
    fn envelope(&self, request: &BodyRequest) -> SignedBodyPayload {
        let payload = serde_json::to_vec(request).unwrap();
        let signature = sign_payload(&self.server_key, &body_request_payload(&payload));
        SignedBodyPayload { payload, signature }
    }
    fn authenticated<T>(&self, body: T, ttl: Duration) -> Request<T> {
        let token = mint_auth_token(
            &self.server_key,
            "body-test-server",
            EMBEDDED_CONTROL_SCOPE,
            ttl,
        )
        .unwrap();
        let mut request = Request::new(body);
        request
            .metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        request
    }
    async fn raw(
        &self,
        envelope: SignedBodyPayload,
        ttl: Duration,
    ) -> std::result::Result<SignedBodyPayload, Status> {
        self.client
            .clone()
            .exchange(self.authenticated(envelope, ttl))
            .await
            .map(Response::into_inner)
    }
    async fn call(&self, operation: BodyOperation) -> Result<BodyEvidence> {
        let request = self.request(operation);
        let envelope = self
            .raw(self.envelope(&request), Duration::from_secs(60))
            .await?;
        verify_payload(
            &self.device_key.verifying_key().to_bytes(),
            &body_reply_payload(&envelope.payload),
            &envelope.signature,
        )?;
        let reply: BodyReply = serde_json::from_slice(&envelope.payload)?;
        ensure!(
            reply.request_id == request.request_id
                && reply.version == 1
                && reply.device_id == request.device_id,
            "signed reply lost request identity"
        );
        Ok(reply.evidence)
    }
    async fn open(&self) -> BodyControlLease {
        let BodyEvidence::Control { state } = self.call(BodyOperation::Inspect).await.unwrap()
        else {
            panic!()
        };
        let BodyEvidence::Control { state } = self
            .call(BodyOperation::Open {
                device_boot_id: state.device_boot_id,
                expected_generation: state.generation,
                server_instance_id: Uuid::new_v4(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        state.lease.unwrap()
    }
    fn command(&self, lease: BodyControlLease, sequence: u64, scope: BodyScope) -> BodyCommand {
        BodyCommand {
            identity: BodyCommandIdentity { lease, sequence },
            plan_id: Uuid::new_v4(),
            source_command_id: format!("test-{sequence}"),
            owner_id: Uuid::new_v4(),
            scope,
            priority: BodyPriority::Normal,
            interruptible: true,
            actions: vec![BodyAction {
                duration_ms: 500,
                intensity: 0.2,
                motion: BodyMotion::SmallNod,
            }],
        }
    }
    async fn finish(self) {
        drop(self.client);
        drop(self.binding_client);
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.playback.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), self.motion.stop())
            .await
            .unwrap()
            .unwrap();
    }
}

async fn with_node<T>(
    test: impl for<'a> FnOnce(&'a Fixture) -> Pin<Box<dyn Future<Output = T> + Send + 'a>>,
) -> T {
    let fixture = Fixture::new().await;
    let result = AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(15),
        test(&fixture),
    ))
    .catch_unwind()
    .await;
    fixture.finish().await;
    match result {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => panic!("body RPC test timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
async fn signed_body_submission_is_observed_without_position_or_replay() {
    with_node(|node| {
        Box::pin(async move {
            let lease = node.open().await;
            let command = node.command(
                lease,
                1,
                BodyScope::Nonvoice {
                    scope_id: Uuid::new_v4(),
                },
            );
            let BodyEvidence::Progress { mut progress } = node
                .call(BodyOperation::Submit {
                    command: command.clone(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            progress.validate(None, &command).unwrap();
            while progress.end.is_none() {
                let BodyEvidence::Progress { progress: next } = node
                    .call(BodyOperation::Observe {
                        identity: command.identity,
                        after_receipt_sequence: progress.receipt_sequence,
                    })
                    .await
                    .unwrap()
                else {
                    panic!()
                };
                if next != progress {
                    next.validate(Some(&progress), &command).unwrap();
                }
                progress = next;
            }
            assert_eq!(progress.end, Some(BodyEnd::Submitted));
            assert!(progress.writer_quiescent && progress.position_confirmed_actions.is_none());
            let duplicate = node
                .call(BodyOperation::Submit {
                    command: command.clone(),
                })
                .await
                .unwrap();
            assert!(matches!(duplicate,BodyEvidence::Progress{progress:p} if p==progress));
            let mut changed = command.clone();
            changed.actions[0].duration_ms = 600;
            assert!(
                node.call(BodyOperation::Submit { command: changed })
                    .await
                    .is_err()
            );
            let control = node.motion.control_state();
            assert!(
                node.call(BodyOperation::Open {
                    device_boot_id: lease.device_boot_id,
                    expected_generation: 0,
                    server_instance_id: lease.server_instance_id
                })
                .await
                .is_err()
            );
            assert_eq!(node.motion.control_state(), control);
        })
    })
    .await;
}

#[tokio::test]
async fn original_voice_scope_and_exact_old_cancel_cannot_reach_replacement() {
    with_node(|node| {
        Box::pin(async move {
            let lease = node.open().await;
            let scope = VoiceScope {
                device_boot_id: lease.device_boot_id,
                session_id: Uuid::new_v4(),
                generation_epoch: 1,
            };
            node.playback.activate(&scope).unwrap();
            let old = node.command(lease, 1, BodyScope::Voice { scope });
            node.call(BodyOperation::Submit {
                command: old.clone(),
            })
            .await
            .unwrap();
            let replacement = VoiceScope {
                generation_epoch: 2,
                ..scope
            };
            node.playback.fence(&replacement, false).unwrap();
            let mut stale = old.clone();
            stale.identity.sequence = 2;
            let BodyEvidence::Progress { progress: rejected } = node
                .call(BodyOperation::Submit {
                    command: stale.clone(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            rejected.validate(None, &stale).unwrap();
            assert!(matches!(rejected.end, Some(BodyEnd::Rejected { .. })));
            assert!(rejected.writer_quiescent && rejected.started_actions == 0);
            let next = node.command(lease, 3, BodyScope::Voice { scope: replacement });
            node.call(BodyOperation::Submit {
                command: next.clone(),
            })
            .await
            .unwrap();
            let BodyEvidence::Progress { progress } = node
                .call(BodyOperation::Cancel {
                    command: old.clone(),
                    reason: "old turn cancelled".into(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            assert!(
                progress.writer_quiescent
                    && matches!(progress.end, Some(BodyEnd::Cancelled { .. }))
            );
            let BodyEvidence::Progress { progress } = node
                .call(BodyOperation::Observe {
                    identity: next.identity,
                    after_receipt_sequence: 0,
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            assert!(!matches!(progress.end, Some(BodyEnd::Cancelled { .. })));
        })
    })
    .await;
}

#[tokio::test]
async fn forged_target_signature_timestamp_and_binding_retirement_prevent_writes() {
    with_node(|node| {
        Box::pin(async move {
            let lease = node.open().await;
            let command = node.command(
                lease,
                1,
                BodyScope::Nonvoice {
                    scope_id: Uuid::new_v4(),
                },
            );
            let request = node.request(BodyOperation::Submit {
                command: command.clone(),
            });
            let mut bad = node.envelope(&request);
            bad.signature[0] ^= 1;
            assert!(node.raw(bad, Duration::from_secs(60)).await.is_err());
            let mut bad = request.clone();
            bad.device_id = "wrong-device".into();
            assert!(
                node.raw(node.envelope(&bad), Duration::from_secs(60))
                    .await
                    .is_err()
            );
            bad = request.clone();
            bad.issued_at_unix_ms -= 31_000;
            assert!(
                node.raw(node.envelope(&bad), Duration::from_secs(60))
                    .await
                    .is_err()
            );
            assert!(node.motion.observe(command.identity).is_err());
            node.call(BodyOperation::Submit {
                command: command.clone(),
            })
            .await
            .unwrap();
            node.binding
                .update_for_test(
                    BindingState::HandoffPending,
                    Some("body-test-server".into()),
                )
                .await;
            assert!(node.motion.control_state().lease.is_none());
            assert!(
                node.call(BodyOperation::Submit {
                    command: command.clone()
                })
                .await
                .is_err()
            );
            let mut observation = node.motion.observe(command.identity).unwrap();
            while observation.snapshot().end.is_none() {
                observation.changed().await.unwrap();
            }
            assert!(matches!(
                observation.snapshot().end,
                Some(BodyEnd::Cancelled { .. })
            ));
        })
    })
    .await;
}

#[tokio::test]
async fn public_status_reachability_and_reconnect_cannot_authorize_unbound_or_foreign_body_control()
{
    with_node(|node| {
        Box::pin(async move {
            let lease = node.open().await;
            let command = node.command(
                lease,
                1,
                BodyScope::Nonvoice {
                    scope_id: Uuid::new_v4(),
                },
            );
            let mut binding = node.binding_client.clone();
            assert!(
                binding
                    .unbind(node.authenticated(
                        UnbindRequest {
                            server_id: "body-test-server".into(),
                        },
                        Duration::from_secs(60),
                    ))
                    .await
                    .unwrap()
                    .into_inner()
                    .success
            );
            for foreign_binding in [false, true] {
                if foreign_binding {
                    let key = SigningKey::from_bytes(&[47; 32]);
                    let public = key.verifying_key().to_bytes();
                    let issued_at = current_unix_timestamp().unwrap();
                    let auth_token = sign_payload(
                        &key,
                        &bind_payload(
                            "other-server",
                            "Other",
                            "http://127.0.0.1:2",
                            &public,
                            issued_at,
                        ),
                    );
                    assert!(
                        binding
                            .bind(BindRequest {
                                server_id: "other-server".into(),
                                server_name: "Other".into(),
                                server_endpoint: "http://127.0.0.1:2".into(),
                                auth_token,
                                server_public_key: public.to_vec(),
                                issued_at,
                            })
                            .await
                            .unwrap()
                            .into_inner()
                            .success
                    );
                }
                // This public RPC remains reachable without any bearer token.
                // Neither a nonempty device ID nor is_bound grants body authority.
                let status = binding.get_binding_status(()).await.unwrap().into_inner();
                assert_eq!(status.device_id, "body-test-device");
                assert_eq!(status.is_bound, foreign_binding);
                assert_eq!(
                    status.bound_server_id,
                    if foreign_binding { "other-server" } else { "" }
                );
                assert!(node.motion.control_state().lease.is_none());
                // Establish a new TCP/gRPC connection after each binding change.
                let mut reconnected = BodyControlClient::connect(node.endpoint.clone())
                    .await
                    .unwrap();
                for operation in [
                    BodyOperation::Inspect,
                    BodyOperation::Open {
                        device_boot_id: lease.device_boot_id,
                        expected_generation: lease.generation,
                        server_instance_id: lease.server_instance_id,
                    },
                    BodyOperation::Submit {
                        command: command.clone(),
                    },
                ] {
                    let request = node.request(operation);
                    let error = reconnected
                        .exchange(
                            node.authenticated(node.envelope(&request), Duration::from_secs(60)),
                        )
                        .await
                        .unwrap_err();
                    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
                }
                assert!(node.motion.observe(command.identity).is_err());
                assert!(node.motion.control_state().lease.is_none());
            }
        })
    })
    .await;
}

#[tokio::test]
async fn replacement_runtime_boot_rejects_original_control_and_command_without_replaying() {
    // Two sequential mock runtimes retain the same fixture device/server keys.
    // This covers the boot contract, not process-kill or controller power loss.
    let original = with_node(|node| {
        Box::pin(async move {
            let command = node.command(
                node.open().await,
                1,
                BodyScope::Nonvoice {
                    scope_id: Uuid::new_v4(),
                },
            );
            node.call(BodyOperation::Submit {
                command: command.clone(),
            })
            .await
            .unwrap();
            let BodyEvidence::Progress { progress } = node
                .call(BodyOperation::Cancel {
                    command: command.clone(),
                    reason: "close first mock runtime".into(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            assert!(progress.writer_quiescent && progress.position_confirmed_actions.is_none());
            command
        })
    })
    .await;
    with_node(move |node| {
        Box::pin(async move {
            let before = node.motion.control_state();
            let lease = original.identity.lease;
            assert_ne!(before.device_boot_id, lease.device_boot_id);
            for operation in [
                BodyOperation::Open {
                    device_boot_id: lease.device_boot_id,
                    expected_generation: before.generation,
                    server_instance_id: lease.server_instance_id,
                },
                BodyOperation::Submit {
                    command: original.clone(),
                },
                BodyOperation::Observe {
                    identity: original.identity,
                    after_receipt_sequence: 0,
                },
                BodyOperation::Cancel {
                    command: original.clone(),
                    reason: "late old cancel".into(),
                },
                BodyOperation::Close {
                    lease,
                    reason: "late old close".into(),
                },
            ] {
                assert!(node.call(operation).await.is_err());
                assert_eq!(node.motion.control_state(), before);
            }
            assert!(node.motion.observe(original.identity).is_err());
            let fresh = node.command(node.open().await, 1, original.scope);
            let BodyEvidence::Progress { progress } = node
                .call(BodyOperation::Submit {
                    command: fresh.clone(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            progress.validate(None, &fresh).unwrap();
            assert_eq!(progress.accepted_actions, 1);
            assert!(progress.position_confirmed_actions.is_none());
        })
    })
    .await;
}

#[tokio::test]
async fn exact_cancel_before_submit_retires_unaccepted_identity_and_keeps_new_work_available() {
    with_node(|node| {
        Box::pin(async move {
            let lease = node.open().await;
            let command = node.command(
                lease,
                1,
                BodyScope::Nonvoice {
                    scope_id: Uuid::new_v4(),
                },
            );
            let prepared = node
                .motion
                .prepare(command.clone(), serde_json::to_vec(&command).unwrap())
                .await
                .unwrap();
            let BodyEvidence::Progress {
                progress: cancelled,
            } = node
                .call(BodyOperation::Cancel {
                    command: command.clone(),
                    reason: "cancel overtook preparation".into(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            cancelled.validate(None, &command).unwrap();
            assert_eq!(cancelled.accepted_actions, 0);
            assert_eq!(cancelled.submitted_frames, 0);
            assert!(
                cancelled.writer_quiescent
                    && matches!(cancelled.end, Some(BodyEnd::Rejected { .. }))
            );
            let delayed = node
                .motion
                .submit(
                    prepared,
                    BodyAuthorization {
                        expires_at: Instant::now() + Duration::from_secs(60),
                    },
                )
                .unwrap()
                .snapshot();
            assert_eq!(delayed, cancelled);
            let BodyEvidence::Progress { progress: replayed } = node
                .call(BodyOperation::Submit {
                    command: command.clone(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(replayed, cancelled);
            let mut forged = command.clone();
            forged.actions[0].intensity = 0.9;
            assert!(
                node.call(BodyOperation::Cancel {
                    command: forged,
                    reason: "different intent".into()
                })
                .await
                .is_err()
            );
            let fresh = node.command(lease, 2, command.scope);
            let BodyEvidence::Progress { progress } = node
                .call(BodyOperation::Submit {
                    command: fresh.clone(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            progress.validate(None, &fresh).unwrap();
            assert_eq!(progress.accepted_actions, 1);
        })
    })
    .await;
}

#[tokio::test]
async fn preparation_failure_has_signed_rejection_and_never_gains_a_retry_permission() {
    with_node(|node| {
        Box::pin(async move {
            let lease = node.open().await;
            let mut command = node.command(
                lease,
                1,
                BodyScope::Nonvoice {
                    scope_id: Uuid::new_v4(),
                },
            );
            command.actions[0].motion = BodyMotion::Animation {
                file: "absent-body-test.anim".into(),
                fps: 30,
                loops: 1,
            };
            let BodyEvidence::Progress { progress } = node
                .call(BodyOperation::Submit {
                    command: command.clone(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            progress.validate(None, &command).unwrap();
            assert!(matches!(progress.end, Some(BodyEnd::Rejected { .. })));
            assert!(progress.writer_quiescent && progress.accepted_actions == 0);
            let BodyEvidence::Progress {
                progress: duplicate,
            } = node
                .call(BodyOperation::Submit {
                    command: command.clone(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(progress, duplicate);
            node.call(BodyOperation::Close {
                lease,
                reason: "retire original lease".into(),
            })
            .await
            .unwrap();
            let BodyEvidence::Progress { progress: old } = node
                .call(BodyOperation::Cancel {
                    command,
                    reason: "query exact old facts".into(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(old, progress);
        })
    })
    .await;
}

#[tokio::test]
async fn token_expiry_cancels_native_plan_without_observer_renewing_it() {
    with_node(|node| {
        Box::pin(async move {
            let state = node.motion.control_state();
            let request = node.request(BodyOperation::Open {
                device_boot_id: state.device_boot_id,
                expected_generation: state.generation,
                server_instance_id: Uuid::new_v4(),
            });
            let envelope = node
                .raw(node.envelope(&request), Duration::from_secs(1))
                .await
                .unwrap();
            let BodyReply {
                evidence: BodyEvidence::Control { state },
                ..
            } = serde_json::from_slice(&envelope.payload).unwrap()
            else {
                panic!()
            };
            let mut command = node.command(
                state.lease.unwrap(),
                1,
                BodyScope::Nonvoice {
                    scope_id: Uuid::new_v4(),
                },
            );
            command.actions = vec![command.actions[0].clone(); 3];
            let request = node.request(BodyOperation::Submit {
                command: command.clone(),
            });
            node.raw(node.envelope(&request), Duration::from_secs(1))
                .await
                .unwrap();
            let mut observation = node.motion.observe(command.identity).unwrap();
            while observation.snapshot().end.is_none() {
                observation.changed().await.unwrap();
            }
            let progress = observation.snapshot();
            assert!(
                matches!(progress.end, Some(BodyEnd::Cancelled { .. }))
                    && progress.writer_quiescent
            );
            assert!(progress.submitted_actions < 3);
            assert!(node.motion.control_state().lease.is_none());
            let BodyEvidence::Progress { progress: queried } = node
                .call(BodyOperation::Observe {
                    identity: command.identity,
                    after_receipt_sequence: 0,
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(progress, queried);
        })
    })
    .await;
}
