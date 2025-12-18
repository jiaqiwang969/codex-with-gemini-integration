use anyhow::Result;
use codex_core::CodexAuth;
use codex_core::ConversationManager;
use codex_core::built_in_model_providers;
use codex_core::openai_models::model_presets::all_model_presets;
use codex_protocol::openai_models::ModelPreset;
use core_test_support::load_default_config_for_test;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_api_key_models() -> Result<()> {
    let codex_home = tempdir()?;
    let config = load_default_config_for_test(&codex_home);
    let manager = ConversationManager::with_models_provider(
        CodexAuth::from_api_key("sk-test"),
        built_in_model_providers()["openai"].clone(),
    );
    let models = manager.list_models(&config).await;

    let expected_models = expected_models_for_api_key();
    assert_eq!(expected_models, models);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_chatgpt_models() -> Result<()> {
    let codex_home = tempdir()?;
    let config = load_default_config_for_test(&codex_home);
    let manager = ConversationManager::with_models_provider(
        CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        built_in_model_providers()["openai"].clone(),
    );
    let models = manager.list_models(&config).await;

    let expected_models = expected_models_for_chatgpt();
    assert_eq!(expected_models, models);

    Ok(())
}

fn expected_models_for_api_key() -> Vec<ModelPreset> {
    expected_builtin_presets()
}

fn expected_models_for_chatgpt() -> Vec<ModelPreset> {
    expected_builtin_presets()
}

fn expected_builtin_presets() -> Vec<ModelPreset> {
    all_model_presets()
        .iter()
        .filter(|preset| preset.show_in_picker)
        .cloned()
        .collect()
}
