//! HTTP skill changes reach real Interaction classification and generation.
use super::skills::{client, definition, response};
use super::*;
use alia_server::channels::adapter::{ChannelAdapter, ChannelCapabilities, ChannelEvent};
use alia_server::event::ServerEvent;
use alia_server::modules::delivery::transport::DeliveryTransport;
use alia_server::modules::memory::source::capture_source_epochs;
use alia_server::providers::adapter::ChatMessage;
use alia_server::state::AppState;
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

const CHANNEL: &str = "classifier-input";
const SKILL: &str = "orchard_compass_fixture";
const PROMPT: &str = "SECRET_DOMAIN_CONTEXT_FOR_FINAL_MODEL_ONLY";
const REPLACEMENT_PROMPT: &str = "REPLACEMENT_CONTEXT_NOT_AUTHORIZED_BY_OLD_CLASSIFICATION";

struct InputChannel;
impl ModuleLifecycle for InputChannel {
    fn name(&self) -> &'static str {
        "classifier-input-fixture"
    }
}
impl DeliveryTransport for InputChannel {
    fn channel_id(&self) -> &str {
        CHANNEL
    }
}
impl ChannelAdapter for InputChannel {
    fn channel_type(&self) -> &'static str {
        "test"
    }
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities::default()
    }
}

#[derive(Clone)]
struct ClassifierProbe {
    role: &'static str,
    selection: Arc<Mutex<String>>,
    classifications: Arc<Mutex<Vec<Value>>>,
    answers: Arc<Mutex<Vec<(&'static str, Vec<ChatMessage>)>>>,
    block_next: Arc<AtomicBool>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
impl ClassifierProbe {
    fn new(name: &str) -> Self {
        Self {
            role: "unassigned",
            selection: Arc::new(Mutex::new(name.into())),
            classifications: Arc::default(),
            answers: Arc::default(),
            block_next: Arc::new(AtomicBool::new(false)),
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
        }
    }
    fn for_role(&self, role: &'static str) -> Self {
        Self {
            role,
            ..self.clone()
        }
    }
    fn latest_candidate(&self, name: &str) -> Option<Value> {
        self.classifications.lock().unwrap().last().unwrap()["available_skills"]
            .as_array()
            .expect("classifier must receive a structured live skill catalog")
            .iter()
            .find(|candidate| candidate["name"] == name)
            .cloned()
    }
    fn last_answer_contains(&self, text: &str) -> bool {
        self.answers
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .1
            .iter()
            .any(|message| message.role == "system" && message.content == text)
    }
    fn last_answer_role(&self) -> &'static str {
        self.answers.lock().unwrap().last().unwrap().0
    }
}
impl ModuleLifecycle for ClassifierProbe {
    fn name(&self) -> &'static str {
        "classifier-model-fixture"
    }
}
impl ClassifierProbe {
    fn service(&self) -> LlmService {
        let fixture = self.clone();
        test_text("ClassifierProbe", move |messages| {
            let fixture = fixture.clone();
            async move {
                if messages
                    .first()
                    .is_some_and(|message| message.content.contains("intent classifier"))
                {
                    anyhow::ensure!(
                        fixture.role == "minor",
                        "classification used the major model"
                    );
                    let content = &messages
                        .iter()
                        .find(|message| message.role == "user")
                        .unwrap()
                        .content;
                    let payload = content
                        .rsplit_once("\n\n")
                        .expect("classifier JSON payload")
                        .1;
                    let payload: Value = serde_json::from_str(payload)?;
                    anyhow::ensure!(
                        !messages
                            .iter()
                            .any(|message| message.content.contains(PROMPT)),
                        "execution prompt leaked into classification"
                    );
                    fixture.classifications.lock().unwrap().push(payload);
                    let selected = fixture.selection.lock().unwrap().clone();
                    fixture.entered.add_permits(1);
                    if fixture.block_next.swap(false, Ordering::SeqCst) {
                        fixture.release.acquire().await?.forget();
                    }
                    return Ok(
                        json!({"route":"skill","skill_name":selected,"confidence":0.99})
                            .to_string(),
                    );
                }
                fixture
                    .answers
                    .lock()
                    .unwrap()
                    .push((fixture.role, messages.to_vec()));
                Ok("A complete classifier fixture answer.".into())
            }
        })
    }
}

fn configure(f: &mut Fixture) {
    f.config.policies.routing.minor_llm_enabled = true;
    f.config.policies.routing.major_llm_enabled = true;
    f.config.policies.conversation.streaming_enabled = false;
    f.config.policies.tools.llm_candidate_filter_enabled = false;
    f.config.policies.cache.enabled = false;
}

async fn turn(state: &AppState, owner: Uuid, text: &str) -> Uuid {
    let mut events = state.event_bus.subscribe();
    let epochs = capture_source_epochs(&state.db, owner).await.unwrap();
    state
        .event_bus
        .publish(ServerEvent::ChannelInput(ChannelEvent::TextMessage {
            channel_id: CHANNEL.into(),
            user_id: owner.to_string(),
            text: text.into(),
            source_received_at: chrono::Utc::now(),
            source_privacy_epoch: epochs.privacy_epoch,
            source_relationship_epoch: epochs.relationship_epoch,
        }));
    let session = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let ServerEvent::ConversationTurnCompleted {
                channel_id,
                user_uuid,
                session_id,
                ..
            } = events.recv().await.unwrap()
                && channel_id == CHANNEL
                && user_uuid == owner
            {
                return session_id;
            }
        }
    })
    .await
    .expect("Interaction did not commit the classifier turn");
    let history = state
        .modules
        .conversation
        .get_history(session, 30)
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .any(|message| message.role == "user" && message.content == text)
    );
    assert_eq!(
        history
            .iter()
            .rev()
            .find(|message| message.role == "assistant")
            .unwrap()
            .content,
        "A complete classifier fixture answer."
    );
    session
}

#[tokio::test]
async fn http_skill_catalog_drives_real_classification_and_final_model_context() {
    with_fixture(|mut f| async move {
        configure(&mut f);
        let probe = ClassifierProbe::new(SKILL);
        let runtime = f.builder().with_minor_llm(probe.for_role("minor").service()).with_major_llm(probe.for_role("major").service())
            .with_channel(InputChannel).build().await.unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let url = format!("http://{}/api/v1/skills", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let mut serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            let item = format!("{url}/{SKILL}");
            let mut body = definition(SKILL, "orchard compass");
            body["description"] = json!("Find the orchard's marked compass points");
            body["prompt_template"] = json!(PROMPT);
            response(client.post(&url).bearer_auth(&token).json(&body), 200).await;
            turn(&state, owner, "Please explain this orchard compass request.").await;
            assert_eq!(probe.latest_candidate(SKILL), Some(json!({
                "name":SKILL,"display_name":"HTTP skill","description":"Find the orchard's marked compass points",
                "category":"test","keywords":["orchard compass"]
            })));
            assert!(probe.last_answer_contains(PROMPT));
            assert_eq!(probe.classifications.lock().unwrap().len(), 1);
            assert_eq!(probe.last_answer_role(), "minor");

            response(client.put(&item).bearer_auth(&token).json(&json!({"description":"Updated compass instructions","intent_patterns":["new compass"]})), 200).await;
            turn(&state, owner, "Describe the revised compass request.").await;
            let updated = probe.latest_candidate(SKILL).unwrap();
            assert_eq!(updated["description"], "Updated compass instructions");
            assert_eq!(updated["keywords"], json!(["new compass"]));
            assert!(probe.last_answer_contains(PROMPT));

            response(client.put(&item).bearer_auth(&token).json(&json!({"enabled":false})), 200).await;
            turn(&state, owner, "Give a normal reply while the compass skill is disabled.").await;
            assert!(probe.latest_candidate(SKILL).is_none());
            assert!(!probe.last_answer_contains(PROMPT));
            assert_eq!(probe.last_answer_role(), "major");
            response(client.put(&item).bearer_auth(&token).json(&json!({"enabled":true})), 200).await;
            response(client.delete(&item).bearer_auth(&token), 200).await;
            turn(&state, owner, "Give a normal reply after deleting the compass skill.").await;
            assert!(probe.latest_candidate(SKILL).is_none());
            assert!(!probe.last_answer_contains(PROMPT));
            assert_eq!(probe.last_answer_role(), "major");
            *probe.selection.lock().unwrap() = "unoffered_classifier_name".into();
            turn(&state, owner, "Give a normal reply to an unknown classifier name.").await;
            assert!(!probe.last_answer_contains(PROMPT));
            assert_eq!(probe.last_answer_role(), "major");
            assert_eq!(probe.classifications.lock().unwrap().len(), 5);
            assert_eq!(probe.answers.lock().unwrap().len(), 5);
        }).catch_unwind().await;
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() { serving.abort(); let _ = serving.await; }
        joined.expect("classifier runtime must join").unwrap().unwrap();
        if let Err(error) = result { std::panic::resume_unwind(error); }
    }).await;
}

#[tokio::test]
async fn classification_in_flight_revalidates_the_selected_skill_and_current_fallback_mode() {
    with_fixture(|mut f| async move {
        configure(&mut f);
        let probe = ClassifierProbe::new(SKILL);
        let runtime = f
            .builder()
            .with_minor_llm(probe.for_role("minor").service())
            .with_major_llm(probe.for_role("major").service())
            .with_channel(InputChannel)
            .build()
            .await
            .unwrap();
        let state = runtime.state().clone();
        let (owner, token) = delivery::account(&state, "user").await;
        let url = format!("http://{}/api/v1/skills", runtime.http_addr());
        let stop = state.cancel_token.clone();
        let _stop_on_drop = stop.clone().drop_guard();
        let mut serving = tokio::spawn(runtime.run());
        let result = AssertUnwindSafe(async {
            let client = client();
            for (index, mutation) in [
                "update",
                "disable",
                "delete",
                "recreate",
                "unrelated",
                "minor_only",
            ]
            .into_iter()
            .enumerate()
            {
                let name = format!("{SKILL}_{mutation}");
                let item = format!("{url}/{name}");
                let mut body = definition(&name, "orchard compass");
                body["prompt_template"] = json!(PROMPT);
                body["preferred_tools"] = json!([]);
                let created =
                    response(client.post(&url).bearer_auth(&token).json(&body), 200).await;
                *probe.selection.lock().unwrap() = name.clone();
                probe.block_next.store(true, Ordering::SeqCst);
                let text = format!("Explain the held orchard request for {mutation}.");
                let change_while_classifying = async {
                    signalled(&probe.entered).await;
                    assert!(
                        probe.latest_candidate(&name).is_some(),
                        "selection was never offered"
                    );
                    match mutation {
                        "update" | "minor_only" => {
                            // Only private execution content changes; candidate metadata stays identical.
                            response(
                                client
                                    .put(&item)
                                    .bearer_auth(&token)
                                    .json(&json!({"prompt_template":REPLACEMENT_PROMPT})),
                                200,
                            )
                            .await;
                            if mutation == "minor_only" {
                                state
                                    .modules
                                    .router
                                    .apply_settings(&std::collections::BTreeMap::from([(
                                        "core.policies.routing.major_llm_enabled".into(),
                                        json!(false),
                                    )]))
                                    .await
                                    .unwrap();
                            }
                        }
                        "disable" => {
                            response(
                                client
                                    .put(&item)
                                    .bearer_auth(&token)
                                    .json(&json!({"enabled":false})),
                                200,
                            )
                            .await;
                        }
                        "delete" => {
                            response(client.delete(&item).bearer_auth(&token), 200).await;
                        }
                        "recreate" => {
                            response(client.delete(&item).bearer_auth(&token), 200).await;
                            // Identical content with a new durable identity must not inherit the old selection.
                            let replacement =
                                response(client.post(&url).bearer_auth(&token).json(&body), 200)
                                    .await;
                            assert_ne!(replacement["id"], created["id"]);
                        }
                        "unrelated" => {
                            response(
                                client.post(&url).bearer_auth(&token).json(&definition(
                                    "unrelated_catalog_change",
                                    "unrelated input",
                                )),
                                200,
                            )
                            .await;
                        }
                        _ => unreachable!(),
                    }
                    probe.release.add_permits(1);
                };
                let ((), _) = tokio::join!(change_while_classifying, turn(&state, owner, &text));
                assert_eq!(
                    probe.last_answer_contains(PROMPT),
                    mutation == "unrelated",
                    "wrong selected-skill disposition for {mutation}"
                );
                assert!(!probe.last_answer_contains(REPLACEMENT_PROMPT));
                assert_eq!(
                    probe.last_answer_role(),
                    if matches!(mutation, "unrelated" | "minor_only") {
                        "minor"
                    } else {
                        "major"
                    }
                );
                assert_eq!(
                    probe.classifications.lock().unwrap().len(),
                    index + 1,
                    "fallback must not classify a second time"
                );
                assert_eq!(probe.answers.lock().unwrap().len(), index + 1);
            }
        })
        .catch_unwind()
        .await;
        stop.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(5), &mut serving).await;
        if joined.is_err() {
            serving.abort();
            let _ = serving.await;
        }
        joined
            .expect("held-classifier runtime must join")
            .unwrap()
            .unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    })
    .await;
}
