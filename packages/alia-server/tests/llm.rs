#![cfg(feature = "llm-openai")]

use alia_server::providers::adapter::ChatMessage;
use alia_server::providers::llm::openai::OpenAiMessage;

#[test]
fn test_message_conversion() {
    let msg = ChatMessage {
        role: "user".to_string(),
        content: "Hello".to_string(),
    };
    let openai_msg = OpenAiMessage::from(&msg);
    assert_eq!(openai_msg.role, "user");
    assert_eq!(openai_msg.content, "Hello");
}
