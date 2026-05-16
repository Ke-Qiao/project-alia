use alia_server::providers::adapter::PersonaProvider;
use alia_server::providers::persona::template::TemplatePersona;
use uuid::Uuid;

#[tokio::test]
async fn test_render_system_prompt() {
    let persona = TemplatePersona::new();

    let result = persona.render_system_prompt(Uuid::new_v4()).await.unwrap();

    assert!(result.contains("Alia"));
    assert!(result.contains("## Core Traits"));
    assert!(!result.contains("## Runtime Context"));
    assert!(!result.contains("## User Memory"));
    assert!(!result.contains("## Skills and Tools"));
    assert!(!result.contains("{{user_presence}}"));
}
