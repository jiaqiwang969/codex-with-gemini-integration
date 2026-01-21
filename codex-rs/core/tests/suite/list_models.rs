use anyhow::Result;
use codex_core::CodexAuth;
use codex_core::ConversationManager;
use codex_core::built_in_model_providers;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use core_test_support::load_default_config_for_test;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_models_returns_api_key_models() -> Result<()> {
    let codex_home = tempdir()?;
    let config = load_default_config_for_test(&codex_home).await;
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
    let config = load_default_config_for_test(&codex_home).await;
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
    let mut models = vec![
        gpt_52_codex(),
        gpt_5_1_codex_max(),
        gpt_5_1_codex_mini(),
        gpt_5_2(),
    ];
    models.extend(gemini_models());
    models
}

fn expected_models_for_chatgpt() -> Vec<ModelPreset> {
    let mut gpt_5_1_codex_max = gpt_5_1_codex_max();
    gpt_5_1_codex_max.is_default = false;
    let mut models = vec![
        gpt_52_codex(),
        gpt_5_1_codex_max,
        gpt_5_1_codex_mini(),
        gpt_5_2(),
    ];
    models.extend(gemini_models());
    models
}

fn gpt_52_codex() -> ModelPreset {
    ModelPreset {
        id: "gpt-5.2-codex".to_string(),
        model: "gpt-5.2-codex".to_string(),
        display_name: "gpt-5.2-codex".to_string(),
        description: "Latest frontier agentic coding model.".to_string(),
        default_reasoning_effort: ReasoningEffort::Medium,
        supported_reasoning_efforts: vec![
            effort(
                ReasoningEffort::Low,
                "Fast responses with lighter reasoning",
            ),
            effort(
                ReasoningEffort::Medium,
                "Balances speed and reasoning depth for everyday tasks",
            ),
            effort(
                ReasoningEffort::High,
                "Greater reasoning depth for complex problems",
            ),
            effort(
                ReasoningEffort::XHigh,
                "Extra high reasoning depth for complex problems",
            ),
        ],
        is_default: true,
        upgrade: None,
        show_in_picker: true,
        supported_in_api: true,
    }
}

fn gpt_5_1_codex_max() -> ModelPreset {
    ModelPreset {
        id: "gpt-5.1-codex-max".to_string(),
        model: "gpt-5.1-codex-max".to_string(),
        display_name: "gpt-5.1-codex-max".to_string(),
        description: "Codex-optimized flagship for deep and fast reasoning.".to_string(),
        default_reasoning_effort: ReasoningEffort::Medium,
        supported_reasoning_efforts: vec![
            effort(
                ReasoningEffort::Low,
                "Fast responses with lighter reasoning",
            ),
            effort(
                ReasoningEffort::Medium,
                "Balances speed and reasoning depth for everyday tasks",
            ),
            effort(
                ReasoningEffort::High,
                "Greater reasoning depth for complex problems",
            ),
            effort(
                ReasoningEffort::XHigh,
                "Extra high reasoning depth for complex problems",
            ),
        ],
        is_default: false,
        upgrade: Some(gpt52_codex_upgrade()),
        show_in_picker: true,
        supported_in_api: true,
    }
}

fn gpt_5_1_codex_mini() -> ModelPreset {
    ModelPreset {
        id: "gpt-5.1-codex-mini".to_string(),
        model: "gpt-5.1-codex-mini".to_string(),
        display_name: "gpt-5.1-codex-mini".to_string(),
        description: "Optimized for codex. Cheaper, faster, but less capable.".to_string(),
        default_reasoning_effort: ReasoningEffort::Medium,
        supported_reasoning_efforts: vec![
            effort(
                ReasoningEffort::Medium,
                "Dynamically adjusts reasoning based on the task",
            ),
            effort(
                ReasoningEffort::High,
                "Maximizes reasoning depth for complex or ambiguous problems",
            ),
        ],
        is_default: false,
        upgrade: Some(gpt52_codex_upgrade()),
        show_in_picker: true,
        supported_in_api: true,
    }
}

fn gpt_5_2() -> ModelPreset {
    ModelPreset {
        id: "gpt-5.2".to_string(),
        model: "gpt-5.2".to_string(),
        display_name: "gpt-5.2".to_string(),
        description:
            "Latest frontier model with improvements across knowledge, reasoning and coding"
                .to_string(),
        default_reasoning_effort: ReasoningEffort::Medium,
        supported_reasoning_efforts: vec![
            effort(
                ReasoningEffort::Low,
                "Balances speed with some reasoning; useful for straightforward queries and short explanations",
            ),
            effort(
                ReasoningEffort::Medium,
                "Provides a solid balance of reasoning depth and latency for general-purpose tasks",
            ),
            effort(
                ReasoningEffort::High,
                "Maximizes reasoning depth for complex or ambiguous problems",
            ),
            effort(
                ReasoningEffort::XHigh,
                "Extra high reasoning for complex problems",
            ),
        ],
        is_default: false,
        upgrade: Some(gpt52_codex_upgrade()),
        show_in_picker: true,
        supported_in_api: true,
    }
}

fn gpt52_codex_upgrade() -> codex_protocol::openai_models::ModelUpgrade {
    codex_protocol::openai_models::ModelUpgrade {
        id: "gpt-5.2-codex".to_string(),
        reasoning_effort_mapping: None,
        migration_config_key: "gpt-5.2-codex".to_string(),
        model_link: Some("https://openai.com/index/introducing-gpt-5-2-codex".to_string()),
        upgrade_copy: Some(
            "Codex is now powered by gpt-5.2-codex, our latest frontier agentic coding model. It is smarter and faster than its predecessors and capable of long-running project-scale work."
                .to_string(),
        ),
    }
}

fn effort(reasoning_effort: ReasoningEffort, description: &str) -> ReasoningEffortPreset {
    ReasoningEffortPreset {
        effort: reasoning_effort,
        description: description.to_string(),
    }
}

fn gemini_models() -> Vec<ModelPreset> {
    vec![
        gemini_3_flash_preview_gemini(),
        gemini_3_pro_preview_codex(),
        gemini_3_pro_image_preview(),
    ]
}

fn gemini_3_flash_preview_gemini() -> ModelPreset {
    ModelPreset {
        id: "gemini-3-flash-preview-gemini".to_string(),
        model: "gemini-3-flash-preview-gemini".to_string(),
        display_name: "gemini-3-flash-preview-gemini".to_string(),
        description: "Google Gemini 3 Flash preview.".to_string(),
        default_reasoning_effort: ReasoningEffort::High,
        supported_reasoning_efforts: vec![
            effort(
                ReasoningEffort::Minimal,
                "Fastest responses with minimal reasoning (Flash-exclusive)",
            ),
            effort(ReasoningEffort::Low, "Lower-cost Gemini thinking"),
            effort(
                ReasoningEffort::Medium,
                "Balanced reasoning depth for general tasks",
            ),
            effort(
                ReasoningEffort::High,
                "Higher-quality Gemini thinking for complex problems",
            ),
        ],
        is_default: false,
        upgrade: None,
        show_in_picker: true,
        supported_in_api: true,
    }
}

fn gemini_3_pro_preview_codex() -> ModelPreset {
    ModelPreset {
        id: "gemini-3-pro-preview-codex".to_string(),
        model: "gemini-3-pro-preview-codex".to_string(),
        display_name: "gemini-3-pro-preview-codex".to_string(),
        description: "Gemini 3 Pro preview with Germini-style system prompt and Codex tooling."
            .to_string(),
        default_reasoning_effort: ReasoningEffort::High,
        supported_reasoning_efforts: vec![
            effort(ReasoningEffort::Low, "Lower-cost Gemini thinking"),
            effort(
                ReasoningEffort::Medium,
                "Balanced reasoning depth for general tasks",
            ),
            effort(
                ReasoningEffort::High,
                "Higher-quality Gemini thinking for complex problems",
            ),
        ],
        is_default: false,
        upgrade: None,
        show_in_picker: true,
        supported_in_api: true,
    }
}

fn gemini_3_pro_image_preview() -> ModelPreset {
    ModelPreset {
        id: "gemini-3-pro-image-preview".to_string(),
        model: "gemini-3-pro-image-preview".to_string(),
        display_name: "gemini-3-pro-image-preview".to_string(),
        description:
            "Gemini 3 Pro image preview for text, image understanding, and image generation."
                .to_string(),
        default_reasoning_effort: ReasoningEffort::Medium,
        supported_reasoning_efforts: vec![effort(
            ReasoningEffort::Medium,
            "Default Gemini reasoning behaviour for image workflows.",
        )],
        is_default: false,
        upgrade: None,
        show_in_picker: true,
        supported_in_api: true,
    }
}
