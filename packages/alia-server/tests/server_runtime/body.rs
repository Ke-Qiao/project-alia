use super::*;
use alia_core::body::*;
use alia_server::modules::body_delivery::*;

#[tokio::test]
async fn body_fact_api_is_owner_only_and_response_cancellation_reaches_admission() {
    with_fixture(|f| async move {
        let runtime = f.builder().build().await.unwrap();
        let address = runtime.http_addr();
        let state = runtime.state().clone();
        let (owner, token) = super::delivery::account(&state, "user").await;
        let (_, other) = super::delivery::account(&state, "admin").await;
        let response = super::delivery::draft(&state, owner).await;
        let repo = BodyDeliveryRepository::new(state.db.clone());
        let command = BodyCommand {
            identity: BodyCommandIdentity {
                lease: BodyControlLease {
                    device_boot_id: Uuid::new_v4(),
                    server_instance_id: Uuid::new_v4(),
                    generation: 1,
                },
                sequence: 1,
            },
            plan_id: Uuid::new_v4(),
            source_command_id: Uuid::new_v4().to_string(),
            owner_id: owner,
            scope: BodyScope::Nonvoice {
                scope_id: response.response_id,
            },
            priority: BodyPriority::Normal,
            interruptible: true,
            actions: vec![BodyAction {
                duration_ms: 500,
                intensity: 0.2,
                motion: BodyMotion::SmallNod,
            }],
        };
        let origin = BodyDeliveryOrigin {
            response_id: response.response_id,
            output_key: "speech_start".into(),
        };
        let BodyDispatch::Fresh {
            delivery_id,
            dispatch_id,
        } = repo
            .claim(&origin, "delivery-test", &command)
            .await
            .unwrap()
        else {
            panic!()
        };
        let text_repo =
            alia_server::modules::delivery::repository::DeliveryRepository::new(state.db.clone());
        let text_owner = Uuid::new_v4();
        let text = text_repo
            .prepare_text_delivery(
                owner,
                alia_server::modules::delivery::types::NewTextDelivery {
                    response_id: response.response_id,
                    output_key: "text:response".into(),
                    content: serde_json::json!({"representation":"plain_utf8"}),
                    identity: Some(alia_core::text_delivery::TextIdentity {
                        text_id: Uuid::new_v4(),
                        channel_instance_id: Uuid::new_v4(),
                    }),
                    owner_id: text_owner,
                },
            )
            .await
            .unwrap();
        let text_permit = text_repo
            .dispatch(owner, text.delivery_id, text_owner)
            .await
            .unwrap()
            .unwrap();
        text_repo.text_start(&text_permit).await.unwrap();
        for (sequence, word) in ["你", "好"].iter().enumerate() {
            text_repo
                .offer_text_chunk(
                    &text_permit,
                    &alia_core::text_delivery::TextChunk {
                        sequence: sequence as u64,
                        utf8_offset: sequence as u64 * 3,
                        text: (*word).into(),
                    },
                )
                .await
                .unwrap();
        }
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let serving = tokio::spawn(runtime.run());
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        for path in [
            format!("body-deliveries/{delivery_id}"),
            format!("responses/{}/body-deliveries", response.response_id),
        ] {
            let url = format!("http://{address}/api/v1/{path}");
            assert_eq!(
                client
                    .get(&url)
                    .bearer_auth(&token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
            assert_eq!(
                client
                    .get(&url)
                    .bearer_auth(&other)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404
            );
            assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
        }
        let invalid = client
            .get(format!(
                "http://{address}/api/v1/body-deliveries/not-a-uuid"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), 400);
        assert_eq!(
            invalid.json::<serde_json::Value>().await.unwrap()["error"]["code"],
            "invalid_input"
        );
        let chunks_url = format!(
            "http://{address}/api/v1/deliveries/{}/text-chunks",
            text.delivery_id
        );
        let chunks = client
            .get(format!("{chunks_url}?limit=1"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(
            chunks,
            serde_json::json!([{"sequence":0,"utf8_offset":0,"text":"你"}])
        );
        let next = client
            .get(format!("{chunks_url}?after=0&limit=1"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(
            next,
            serde_json::json!([{"sequence":1,"utf8_offset":3,"text":"好"}])
        );
        assert_eq!(
            client
                .get(&chunks_url)
                .bearer_auth(&other)
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        for query in ["limit=0", "limit=257", "after=not-a-number"] {
            assert_eq!(
                client
                    .get(format!("{chunks_url}?{query}"))
                    .bearer_auth(&token)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                400
            );
        }
        assert!(repo.authorize(delivery_id, dispatch_id).await.unwrap());
        assert_eq!(
            client
                .post(format!(
                    "http://{address}/api/v1/responses/{}/cancel",
                    response.response_id
                ))
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        assert!(!repo.authorize(delivery_id, dispatch_id).await.unwrap());
        // The request alone cannot assert a physical cancellation checkpoint.
        let facts = client
            .get(format!(
                "http://{address}/api/v1/body-deliveries/{delivery_id}"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(facts["status"], "sending");
        assert!(facts["progress"].is_null());
        state
            .db
            .execute_unprepared(&format!(
                "UPDATE users SET status='disabled' WHERE id='{owner}'"
            ))
            .await
            .unwrap();
        let denied = client
            .get(&chunks_url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status();
        assert!(denied == 401 || denied == 403);
        state
            .db
            .execute_unprepared(&format!(
                "UPDATE users SET status='active' WHERE id='{owner}'"
            ))
            .await
            .unwrap();
        state
            .db
            // Keep this schema's relation so search_path cannot fall through to
            // a public table installed by another migration test.
            .execute_unprepared(
                "ALTER TABLE body_deliveries RENAME COLUMN delivery_id TO unavailable_delivery_id",
            )
            .await
            .unwrap();
        let unavailable = client
            .get(format!(
                "http://{address}/api/v1/body-deliveries/{delivery_id}"
            ))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(unavailable.status(), 503);
        let body = unavailable.json::<serde_json::Value>().await.unwrap();
        assert_eq!(body["error"]["code"], "delivery_unavailable");
        assert!(!body.to_string().contains("relation") && !body.to_string().contains("postgresql"));
        stop.cancel();
        serving.await.unwrap().unwrap();
    })
    .await;
}
