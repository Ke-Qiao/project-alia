use alia_server::providers::adapter::{PersonaProvider, PromptVars};
use alia_server::providers::persona::template::TemplatePersona;
use uuid::Uuid;

#[tokio::test]
async fn test_render_system_prompt() {
    let persona = TemplatePersona::new();
    let vars = PromptVars {
        core_memories: vec!["用户喜欢喝咖啡".to_string()],
        user_preferences: vec![("温度".to_string(), "26°C".to_string())],
        current_time: "2025-05-07 14:30".to_string(),
        user_presence: "在家".to_string(),
        device_status_summary: "空调运行中".to_string(),
        available_skills: vec!["控制空调".to_string(), "播放音乐".to_string()],
    };

    let result = persona
        .render_system_prompt(Uuid::new_v4(), vars)
        .await
        .unwrap();

    assert!(result.contains("Alia"));
    assert!(result.contains("用户喜欢喝咖啡"));
    assert!(result.contains("26°C"));
}
