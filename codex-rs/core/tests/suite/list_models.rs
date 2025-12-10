use anyhow::Result;
use codex_core::CodexAuth;
use codex_core::ConversationManager;
use codex_core::openai_models::model_presets::all_model_presets;
use codex_protocol::openai_models::ModelPreset;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_api_key_models() -> Result<()> {
    let manager = ConversationManager::with_auth(CodexAuth::from_api_key("sk-test"));
    let models = manager.list_models().await;

    let expected_models = expected_models_for_api_key();
    assert_eq!(expected_models, models);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_chatgpt_models() -> Result<()> {
    let manager =
        ConversationManager::with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    let models = manager.list_models().await;

    let expected_models = expected_models_for_chatgpt();
    assert_eq!(expected_models, models);

    Ok(())
}

fn expected_models_for_api_key() -> Vec<ModelPreset> {
    all_model_presets()
        .iter()
        .filter(|preset| preset.show_in_picker)
        .cloned()
        .collect()
}

fn expected_models_for_chatgpt() -> Vec<ModelPreset> {
    expected_models_for_api_key()
}
