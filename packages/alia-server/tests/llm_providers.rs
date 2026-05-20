use alia_server::module::factory::{create_embedding, create_llm};

#[test]
fn creates_provider_neutral_llm_adapters() {
    assert_eq!(create_llm("openai").name(), "openai");
    assert_eq!(create_llm("anthropic").name(), "anthropic");
    assert_eq!(create_llm("gemini").name(), "gemini");
}

#[test]
fn unknown_llm_name_falls_back_to_noop() {
    assert_eq!(create_llm("unknown").name(), "noop_llm");
}

#[test]
fn unknown_embedding_name_falls_back_to_noop() {
    assert_eq!(create_embedding("unknown").name(), "noop_embedding");
}
