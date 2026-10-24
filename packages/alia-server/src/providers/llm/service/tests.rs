use super::*;

#[tokio::test]
async fn scripted_text_uses_rig_stream_and_obeys_lifecycle() {
    let model = LlmService::test_text("test", |_| async { Ok("hello".into()) });
    assert_eq!(model.text(&[]).await.unwrap(), "hello");
    let tokens = model
        .text_stream(&[])
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        tokens
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .concat(),
        "hello"
    );
    model.stop().await.unwrap();
    assert!(model.text(&[]).await.is_err());
}
