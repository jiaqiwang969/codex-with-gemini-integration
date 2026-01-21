use crate::app_backtrack::BacktrackState;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::ApprovalRequest;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::ChatWidgetInit;
use crate::chatwidget::DelegateDisplayLabel;
use crate::clipboard_copy;
use crate::custom_terminal::Frame;
use crate::diff_render::DiffSummary;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::file_search::FileSearchManager;
use crate::history_cell::HistoryCell;
use crate::history_cell::UserHistoryCell;
use crate::model_migration::ModelMigrationOutcome;
use crate::model_migration::migration_copy_for_models;
use crate::model_migration::run_model_migration_prompt;
use crate::pager_overlay::Overlay;
use crate::render::highlight::highlight_bash_to_lines;
use crate::render::renderable::Renderable;
use crate::resume_picker::ResumeSelection;
use crate::session_bar::SessionBar;
use crate::tui;
use crate::tui::TuiEvent;
use crate::tui::scrolling::TranscriptLineMeta;
use crate::tui::scrolling::TranscriptScroll;
use crate::update_action::UpdateAction;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_line;
use crate::wrapping::word_wrap_lines_borrowed;
use codex_ansi_escape::ansi_escape_line;
use codex_app_server_protocol::AuthMode;
use codex_core::AuthManager;
use codex_core::ConversationManager;
use codex_core::config::Config;
use codex_core::config::edit::ConfigEditsBuilder;
#[cfg(target_os = "windows")]
use codex_core::features::Feature;
use codex_core::openai_models::model_presets::HIDE_GPT_5_1_CODEX_MAX_MIGRATION_PROMPT_CONFIG;
use codex_core::openai_models::model_presets::HIDE_GPT5_1_MIGRATION_PROMPT_CONFIG;
use codex_core::openai_models::models_manager::ModelsManager;
use codex_core::protocol::EventMsg;
use codex_core::protocol::FinalOutput;
use codex_core::protocol::Op;
use codex_core::protocol::SessionConfiguredEvent;
use codex_core::protocol::SessionSource;
use codex_core::protocol::TokenUsage;
use codex_multi_agent::AgentId;
use codex_multi_agent::AgentOrchestrator;
use codex_multi_agent::DelegateEvent;
use codex_multi_agent::DelegateSessionMode;
use codex_multi_agent::DelegateSessionSummary;
use codex_multi_agent::DetachedRunSummary;
use codex_multi_agent::delegate_tool_adapter;
use codex_protocol::ConversationId;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ModelUpgrade;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_tumix::Round1Result;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::MouseButton;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use tokio::select;
use tokio::sync::mpsc::unbounded_channel;
use tokio::task::JoinHandle;
use unicode_width::UnicodeWidthStr;

#[cfg(not(debug_assertions))]
use crate::history_cell::UpdateAvailableHistoryCell;

const GPT_5_1_MIGRATION_AUTH_MODES: [AuthMode; 2] = [AuthMode::ChatGPT, AuthMode::ApiKey];
const GPT_5_1_CODEX_MIGRATION_AUTH_MODES: [AuthMode; 1] = [AuthMode::ChatGPT];

#[derive(Debug, Clone)]
pub struct AppExitInfo {
    pub token_usage: TokenUsage,
    pub conversation_id: Option<ConversationId>,
    pub update_action: Option<UpdateAction>,
    /// ANSI-styled transcript lines to print after the TUI exits.
    ///
    /// These lines are rendered against the same width as the final TUI
    /// viewport and include styling (colors, bold, etc.) so that scrollback
    /// preserves the visual structure of the on-screen transcript.
    pub session_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PanelFocus {
    Sessions,
    Chat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LayoutMode {
    Normal,
    #[allow(dead_code)]
    Collapsed,
}

/// Content-relative selection within the inline transcript viewport.
///
/// Selection endpoints are expressed in terms of flattened, wrapped transcript
/// line indices and columns, so the highlight tracks logical conversation
/// content even when the viewport scrolls or the terminal is resized.
#[derive(Debug, Clone, Copy, Default)]
struct TranscriptSelection {
    anchor: Option<TranscriptSelectionPoint>,
    head: Option<TranscriptSelectionPoint>,
}

/// A single endpoint of a transcript selection.
///
/// `line_index` is an index into the flattened wrapped transcript lines, and
/// `column` is a zero-based column offset within that visual line, counted from
/// the first content column to the right of the transcript gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TranscriptSelectionPoint {
    line_index: usize,
    column: u16,
}

fn session_summary(
    token_usage: TokenUsage,
    conversation_id: Option<ConversationId>,
) -> Option<SessionSummary> {
    if token_usage.is_zero() {
        return None;
    }

    let usage_line = FinalOutput::from(token_usage).to_string();
    let resume_command =
        conversation_id.map(|conversation_id| format!("codex resume {conversation_id}"));
    Some(SessionSummary {
        usage_line,
        resume_command,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionSummary {
    usage_line: String,
    resume_command: Option<String>,
}

fn should_show_model_migration_prompt(
    current_model: &str,
    target_model: &str,
    hide_prompt_flag: bool,
    available_models: &[ModelPreset],
) -> bool {
    if target_model == current_model || hide_prompt_flag {
        return false;
    }

    available_models
        .iter()
        .filter(|preset| preset.upgrade.is_some())
        .any(|preset| preset.model == current_model)
}

fn migration_prompt_hidden(config: &Config, migration_config_key: &str) -> bool {
    match migration_config_key {
        HIDE_GPT_5_1_CODEX_MAX_MIGRATION_PROMPT_CONFIG => config
            .notices
            .hide_gpt_5_1_codex_max_migration_prompt
            .unwrap_or(false),
        HIDE_GPT5_1_MIGRATION_PROMPT_CONFIG => {
            config.notices.hide_gpt5_1_migration_prompt.unwrap_or(false)
        }
        _ => false,
    }
}

async fn handle_model_migration_prompt_if_needed(
    tui: &mut tui::Tui,
    config: &mut Config,
    model: &str,
    app_event_tx: &AppEventSender,
    models_manager: Arc<ModelsManager>,
    auth_mode: Option<AuthMode>,
) -> Option<AppExitInfo> {
    let available_models = models_manager.list_models(config).await;
    let upgrade = available_models
        .iter()
        .find(|preset| preset.model == model)
        .and_then(|preset| preset.upgrade.as_ref());

    if let Some(ModelUpgrade {
        id: target_model,
        reasoning_effort_mapping,
        migration_config_key,
        ..
    }) = upgrade
    {
        if !migration_prompt_allows_auth_mode(auth_mode, migration_config_key.as_str()) {
            return None;
        }

        let hide_prompt = migration_prompt_hidden(config, migration_config_key.as_str());

        let target_model = target_model.to_string();
        if !should_show_model_migration_prompt(model, &target_model, hide_prompt, &available_models)
        {
            return None;
        }

        let current_preset = available_models.iter().find(|preset| preset.model == model);
        let target_preset = available_models
            .iter()
            .find(|preset| preset.model == target_model);
        let target_display_name = target_preset
            .map(|preset| preset.display_name.clone())
            .unwrap_or_else(|| target_model.clone());
        let heading_label = if target_display_name == model {
            target_model.clone()
        } else {
            target_display_name.clone()
        };
        let target_description = target_preset.and_then(|preset| {
            if preset.description.is_empty() {
                None
            } else {
                Some(preset.description.clone())
            }
        });
        let can_opt_out = current_preset.is_some();
        let prompt_copy = migration_copy_for_models(
            model,
            &target_model,
            heading_label,
            target_description,
            can_opt_out,
        );

        match run_model_migration_prompt(tui, prompt_copy).await {
            ModelMigrationOutcome::Accepted => {
                app_event_tx.send(AppEvent::PersistModelMigrationPromptAcknowledged {
                    migration_config: migration_config_key.to_string(),
                });
                config.model = Some(target_model.clone());

                let mapped_effort = if let Some(reasoning_effort_mapping) = reasoning_effort_mapping
                    && let Some(reasoning_effort) = config.model_reasoning_effort
                {
                    reasoning_effort_mapping
                        .get(&reasoning_effort)
                        .cloned()
                        .or(config.model_reasoning_effort)
                } else {
                    config.model_reasoning_effort
                };

                config.model_reasoning_effort = mapped_effort;

                app_event_tx.send(AppEvent::UpdateModel(target_model.clone()));
                app_event_tx.send(AppEvent::UpdateReasoningEffort(mapped_effort));
                app_event_tx.send(AppEvent::PersistModelSelection {
                    model: target_model.clone(),
                    effort: mapped_effort,
                });
            }
            ModelMigrationOutcome::Rejected => {
                app_event_tx.send(AppEvent::PersistModelMigrationPromptAcknowledged {
                    migration_config: migration_config_key.to_string(),
                });
            }
            ModelMigrationOutcome::Exit => {
                return Some(AppExitInfo {
                    token_usage: TokenUsage::default(),
                    conversation_id: None,
                    update_action: None,
                    session_lines: Vec::new(),
                });
            }
        }
    }

    None
}

pub(crate) struct App {
    pub(crate) server: Arc<ConversationManager>,
    pub(crate) app_event_tx: AppEventSender,
    pub(crate) chat_widget: ChatWidget,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) models_manager: Arc<ModelsManager>,
    pub(crate) delegate_orchestrator: Arc<AgentOrchestrator>,

    /// Config is stored here so we can recreate ChatWidgets as needed.
    pub(crate) config: Config,
    pub(crate) current_model: String,
    pub(crate) active_profile: Option<String>,

    pub(crate) file_search: FileSearchManager,

    pub(crate) transcript_cells: Vec<Arc<dyn HistoryCell>>,

    transcript_scroll: TranscriptScroll,
    transcript_selection: TranscriptSelection,
    transcript_view_top: usize,
    transcript_total_lines: usize,

    // Session panel components
    pub(crate) session_bar: SessionBar,
    pub(crate) panel_focus: PanelFocus,
    #[allow(dead_code)]
    pub(crate) layout_mode: LayoutMode,

    // Pager overlay state (Transcript or Static like Diff)
    pub(crate) overlay: Option<Overlay>,
    pub(crate) deferred_history_lines: Vec<Line<'static>>,
    has_emitted_history_lines: bool,

    pub(crate) enhanced_keys_supported: bool,

    /// Controls the animation thread that sends CommitTick events.
    pub(crate) commit_anim_running: Arc<AtomicBool>,

    // Esc-backtracking state grouped
    pub(crate) backtrack: crate::app_backtrack::BacktrackState,
    cxresume_cache: Option<crate::cxresume_picker_widget::PickerState>,
    cxresume_idle: CxresumeIdleLoader,
    pub(crate) feedback: codex_feedback::CodexFeedback,
    delegate_sessions: HashMap<String, DelegateSessionState>,
    active_delegate: Option<String>,
    active_delegate_summary: Option<DelegateSessionSummary>,
    primary_chat_backup: Option<ChatWidget>,
    /// Set when the user confirms an update; propagated on exit.
    pub(crate) pending_update_action: Option<UpdateAction>,
    delegate_tree: DelegateTree,
    delegate_status_owner: Option<String>,
    /// Ignore the next ShutdownComplete event when we're intentionally
    /// stopping a conversation (e.g., before starting a new one).
    suppress_shutdown_complete: bool,
    // One-shot suppression of the next world-writable scan after user confirmation.
    skip_world_writable_scan_once: bool,
}

#[derive(Default)]
struct DelegateTree {
    nodes: HashMap<String, DelegateNode>,
    roots: Vec<String>,
}

struct DelegateNode {
    agent_id: AgentId,
    parent: Option<String>,
    children: Vec<String>,
}

#[derive(Clone)]
struct DelegateDisplay {
    depth: usize,
    label: DelegateDisplayLabel,
}

impl DelegateTree {
    fn insert(
        &mut self,
        run_id: String,
        agent_id: AgentId,
        parent: Option<String>,
    ) -> DelegateDisplay {
        if let Some(parent_id) = parent.as_ref() {
            if let Some(parent_node) = self.nodes.get_mut(parent_id) {
                parent_node.children.push(run_id.clone());
            }
        } else {
            self.roots.push(run_id.clone());
        }

        self.nodes.insert(
            run_id.clone(),
            DelegateNode {
                agent_id: agent_id.clone(),
                parent: parent.clone(),
                children: Vec::new(),
            },
        );

        self.display_for(&run_id, &agent_id)
    }

    fn display_for(&self, run_id: &str, agent_id: &AgentId) -> DelegateDisplay {
        let depth = self.depth_of(run_id).unwrap_or(0);
        let base_label = if depth == 0 {
            format!("↳ #{}", agent_id.as_str())
        } else {
            let indent = "  ".repeat(depth);
            format!("{indent}↳ #{}", agent_id.as_str())
        };
        DelegateDisplay {
            depth,
            label: DelegateDisplayLabel { depth, base_label },
        }
    }

    fn depth_of(&self, run_id: &str) -> Option<usize> {
        let mut depth = 0;
        let mut current = run_id;
        while let Some(node) = self.nodes.get(current) {
            if let Some(parent) = node.parent.as_ref() {
                depth += 1;
                current = parent;
            } else {
                break;
            }
        }
        if self.nodes.contains_key(run_id) || self.roots.iter().any(|r| r == run_id) {
            Some(depth)
        } else {
            None
        }
    }

    fn remove(&mut self, run_id: &str) {
        if let Some(node) = self.nodes.remove(run_id) {
            if let Some(parent_id) = node.parent {
                if let Some(parent_node) = self.nodes.get_mut(&parent_id) {
                    parent_node.children.retain(|child| child != run_id);
                }
            } else {
                self.roots.retain(|root| root != run_id);
            }
        }
    }

    fn first_active_root(&self) -> Option<(String, AgentId)> {
        for run_id in &self.roots {
            if let Some(node) = self.nodes.get(run_id) {
                return Some((run_id.clone(), node.agent_id.clone()));
            }
        }
        None
    }
}

impl App {
    async fn shutdown_current_conversation(&mut self) {
        if let Some(conversation_id) = self.chat_widget.conversation_id() {
            self.suppress_shutdown_complete = true;
            self.chat_widget.submit_op(Op::Shutdown);
            self.server.remove_conversation(&conversation_id).await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        tui: &mut tui::Tui,
        auth_manager: Arc<AuthManager>,
        delegate_orchestrator: Arc<AgentOrchestrator>,
        mut config: Config,
        active_profile: Option<String>,
        initial_prompt: Option<String>,
        initial_images: Vec<PathBuf>,
        resume_selection: ResumeSelection,
        feedback: codex_feedback::CodexFeedback,
    ) -> Result<AppExitInfo> {
        use tokio_stream::StreamExt;
        let (app_event_tx, mut app_event_rx) = unbounded_channel();
        let app_event_tx = AppEventSender::new(app_event_tx);

        let models_manager = Arc::new(ModelsManager::new(auth_manager.clone()));
        let resolved_model = models_manager.get_model(&config.model, &config).await;
        let auth_mode = auth_manager.auth().map(|auth| auth.mode);
        let exit_info = handle_model_migration_prompt_if_needed(
            tui,
            &mut config,
            resolved_model.as_str(),
            &app_event_tx,
            models_manager.clone(),
            auth_mode,
        )
        .await;
        if let Some(exit_info) = exit_info {
            return Ok(exit_info);
        }
        let resolved_model = models_manager.get_model(&config.model, &config).await;
        let model_family = models_manager
            .construct_model_family(resolved_model.as_str(), &config)
            .await;
        let current_model = model_family.get_model_slug().to_string();
        let is_first_run = matches!(
            resume_selection,
            ResumeSelection::StartFresh | ResumeSelection::Exit
        );

        // Wire up delegate orchestrator (custom multi-agent integration).
        let delegate_adapter = delegate_tool_adapter(delegate_orchestrator.clone());
        let mut delegate_event_rx = delegate_orchestrator.subscribe().await;
        let delegate_app_event_tx = app_event_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = delegate_event_rx.recv().await {
                delegate_app_event_tx.send(AppEvent::DelegateUpdate(event));
            }
        });

        let conversation_manager = Arc::new(ConversationManager::with_delegate(
            auth_manager.clone(),
            SessionSource::Cli,
            Some(delegate_adapter.clone()),
        ));

        let enhanced_keys_supported = tui.enhanced_keys_supported();

        let mut chat_widget = match resume_selection {
            ResumeSelection::StartFresh | ResumeSelection::Exit => {
                let init = crate::chatwidget::ChatWidgetInit {
                    config: config.clone(),
                    frame_requester: tui.frame_requester(),
                    app_event_tx: app_event_tx.clone(),
                    initial_prompt: initial_prompt.clone(),
                    initial_images: initial_images.clone(),
                    enhanced_keys_supported,
                    auth_manager: auth_manager.clone(),
                    models_manager: models_manager.clone(),
                    feedback: feedback.clone(),
                    is_first_run,
                    model_family: model_family.clone(),
                };
                ChatWidget::new(init, conversation_manager.clone())
            }
            ResumeSelection::Resume(path) => {
                let resumed = conversation_manager
                    .resume_conversation_from_rollout(
                        config.clone(),
                        path.clone(),
                        auth_manager.clone(),
                    )
                    .await
                    .wrap_err_with(|| {
                        format!("Failed to resume session from {}", path.display())
                    })?;
                let init = crate::chatwidget::ChatWidgetInit {
                    config: config.clone(),
                    frame_requester: tui.frame_requester(),
                    app_event_tx: app_event_tx.clone(),
                    initial_prompt: initial_prompt.clone(),
                    initial_images: initial_images.clone(),
                    enhanced_keys_supported,
                    auth_manager: auth_manager.clone(),
                    models_manager: models_manager.clone(),
                    feedback: feedback.clone(),
                    is_first_run,
                    model_family: model_family.clone(),
                };
                ChatWidget::new_from_existing(
                    init,
                    resumed.conversation_id.to_string(),
                    resumed.conversation,
                    resumed.session_configured,
                )
            }
        };

        chat_widget.maybe_prompt_windows_sandbox_enable();

        let file_search = FileSearchManager::new(config.cwd.clone(), app_event_tx.clone());
        #[cfg(not(debug_assertions))]
        let upgrade_version = crate::updates::get_upgrade_version(&config);

        let session_bar = SessionBar::new(config.cwd.clone(), config.codex_home.clone());

        let mut app = Self {
            server: conversation_manager,
            app_event_tx,
            chat_widget,
            auth_manager: auth_manager.clone(),
            models_manager: models_manager.clone(),
            delegate_orchestrator,
            config,
            current_model,
            active_profile,
            file_search,
            enhanced_keys_supported,
            transcript_cells: Vec::new(),
            transcript_scroll: TranscriptScroll::default(),
            transcript_selection: TranscriptSelection::default(),
            transcript_view_top: 0,
            transcript_total_lines: 0,
            overlay: None,
            deferred_history_lines: Vec::new(),
            has_emitted_history_lines: false,
            commit_anim_running: Arc::new(AtomicBool::new(false)),
            backtrack: BacktrackState::default(),
            cxresume_cache: None,
            cxresume_idle: CxresumeIdleLoader::new(Duration::from_secs(2)),
            feedback: feedback.clone(),
            delegate_sessions: HashMap::new(),
            active_delegate: None,
            active_delegate_summary: None,
            primary_chat_backup: None,
            pending_update_action: None,
            delegate_tree: DelegateTree::default(),
            delegate_status_owner: None,
            suppress_shutdown_complete: false,
            skip_world_writable_scan_once: false,
            session_bar,
            panel_focus: PanelFocus::Chat,
            layout_mode: LayoutMode::Normal,
        };

        app.cxresume_idle.trigger_immediate(&app.app_event_tx);

        // On startup, if Agent mode (workspace-write) or ReadOnly is active, warn about world-writable dirs on Windows.
        #[cfg(target_os = "windows")]
        {
            let should_check = codex_core::get_platform_sandbox().is_some()
                && matches!(
                    app.config.sandbox_policy,
                    codex_core::protocol::SandboxPolicy::WorkspaceWrite { .. }
                        | codex_core::protocol::SandboxPolicy::ReadOnly
                )
                && !app
                    .config
                    .notices
                    .hide_world_writable_warning
                    .unwrap_or(false);
            if should_check {
                let cwd = app.config.cwd.clone();
                let env_map: std::collections::HashMap<String, String> = std::env::vars().collect();
                let tx = app.app_event_tx.clone();
                let logs_base_dir = app.config.codex_home.clone();
                let sandbox_policy = app.config.sandbox_policy.clone();
                Self::spawn_world_writable_scan(cwd, env_map, logs_base_dir, sandbox_policy, tx);
            }
        }

        #[cfg(not(debug_assertions))]
        if let Some(latest_version) = upgrade_version {
            app.handle_event(
                tui,
                AppEvent::InsertHistoryCell(Box::new(UpdateAvailableHistoryCell::new(
                    latest_version,
                    crate::update_action::get_update_action(),
                ))),
            )
            .await?;
        }

        let tui_events = tui.event_stream();
        tokio::pin!(tui_events);

        tui.frame_requester().schedule_frame();

        while select! {
            Some(event) = app_event_rx.recv() => {
                app.handle_event(tui, event).await?
            }
            Some(event) = tui_events.next() => {
                app.handle_tui_event(tui, event).await?
            }
        } {}
        let width = tui.terminal.last_known_screen_size.width;
        let session_lines = if width == 0 {
            Vec::new()
        } else {
            let (lines, line_meta) = Self::build_transcript_lines(&app.transcript_cells, width);
            let is_user_cell: Vec<bool> = app
                .transcript_cells
                .iter()
                .map(|cell| cell.as_any().is::<UserHistoryCell>())
                .collect();
            Self::render_lines_to_ansi(&lines, &line_meta, &is_user_cell, width)
        };
        tui.terminal.clear()?;
        Ok(AppExitInfo {
            token_usage: app.token_usage(),
            conversation_id: app.chat_widget.conversation_id(),
            update_action: app.pending_update_action,
            session_lines,
        })
    }

    pub(crate) fn open_or_refresh_session_picker(&mut self, tui: &mut tui::Tui) {
        if let Some(Overlay::SessionPicker(picker)) = self.overlay.as_mut() {
            if let Err(err) = picker.refresh_sessions() {
                self.chat_widget
                    .add_error_message(format!("Failed to refresh sessions: {err}"));
                tracing::warn!("Failed to refresh session picker: {}", err);
            }
            tui.frame_requester().schedule_frame();
            return;
        }

        let overlay = if let Some(state) = self.cxresume_cache.clone() {
            Ok(Overlay::SessionPicker(Box::new(
                crate::pager_overlay::SessionPickerOverlay::from_state(state),
            )))
        } else {
            crate::cxresume_picker_widget::create_session_picker_overlay()
        };

        match overlay {
            Ok(overlay) => {
                let _ = tui.enter_alt_screen();
                self.overlay = Some(overlay);
                if let Some(state) = self
                    .overlay
                    .as_ref()
                    .and_then(super::pager_overlay::Overlay::session_picker_state)
                {
                    self.update_cxresume_cache(state);
                }
                tui.frame_requester().schedule_frame();
            }
            Err(err) => {
                self.chat_widget
                    .add_error_message(format!("Failed to load sessions: {err}"));
                tracing::warn!("Failed to create session picker: {}", err);
            }
        }
    }

    pub(crate) fn update_cxresume_cache(
        &mut self,
        state: crate::cxresume_picker_widget::PickerState,
    ) {
        self.cxresume_cache = Some(state);
    }

    pub(crate) fn reset_cxresume_idle(&mut self) {
        self.cxresume_idle.on_user_activity(&self.app_event_tx);
    }

    pub(crate) async fn handle_tui_event(
        &mut self,
        tui: &mut tui::Tui,
        event: TuiEvent,
    ) -> Result<bool> {
        if matches!(event, TuiEvent::Key(_) | TuiEvent::Paste(_)) {
            self.reset_cxresume_idle();
        }

        if self.overlay.is_some() {
            let _ = self.handle_backtrack_overlay_event(tui, event).await?;
        } else {
            match event {
                TuiEvent::Key(key_event) => {
                    self.handle_key_event(tui, key_event).await;
                }
                TuiEvent::Paste(pasted) => {
                    // Many terminals convert newlines to \r when pasting (e.g., iTerm2),
                    // but tui-textarea expects \n. Normalize CR to LF.
                    // [tui-textarea]: https://github.com/rhysd/tui-textarea/blob/4d18622eeac13b309e0ff6a55a46ac6706da68cf/src/textarea.rs#L782-L783
                    // [iTerm2]: https://github.com/gnachman/iTerm2/blob/5d0c0d9f68523cbd0494dad5422998964a2ecd8d/sources/iTermPasteHelper.m#L206-L216
                    let pasted = pasted.replace("\r", "\n");
                    self.chat_widget.handle_paste(pasted);
                }
                TuiEvent::Mouse(mouse_event) => {
                    self.handle_mouse_event(tui, mouse_event);
                }
                TuiEvent::Draw => {
                    self.chat_widget.maybe_post_pending_notification(tui);
                    if self
                        .chat_widget
                        .handle_paste_burst_tick(tui.frame_requester())
                    {
                        return Ok(true);
                    }
                    // Update session bar with current conversation ID and status derived from ChatWidget
                    let current_conv_id =
                        self.chat_widget.conversation_id().map(|id| id.to_string());
                    self.session_bar
                        .set_current_session(current_conv_id.clone());
                    if let Some(id) = current_conv_id {
                        self.session_bar
                            .set_session_status(id, self.chat_widget.sidebar_status());
                    }

                    // Clone is needed here due to borrow checker constraints:
                    // the closure captures `self` mutably for render_transcript_cells,
                    // but we also need to read transcript_cells. Using Arc means only
                    // pointer counts are cloned, not the underlying HistoryCell data.
                    let cells = self.transcript_cells.clone();
                    tui.draw(tui.terminal.size()?.height, |frame| {
                        let session_height = 4u16.min(frame.area().height);
                        let available_for_chat = frame.area().height.saturating_sub(session_height);
                        let chat_height = self
                            .chat_widget
                            .desired_height(frame.area().width)
                            .min(available_for_chat);

                        let chat_top = self.render_transcript_cells(
                            frame,
                            &cells,
                            chat_height.saturating_add(session_height),
                        );
                        let session_area = Rect {
                            x: frame.area().x,
                            y: frame.area().bottom().saturating_sub(session_height),
                            width: frame.area().width,
                            height: session_height,
                        };
                        let chat_max_height = session_area.y.saturating_sub(chat_top);
                        let chat_area = Rect {
                            x: frame.area().x,
                            y: chat_top,
                            width: frame.area().width,
                            height: chat_height.min(chat_max_height),
                        };
                        self.chat_widget.render(chat_area, frame.buffer);
                        let chat_bottom = chat_area.y.saturating_add(chat_area.height);
                        if chat_bottom < frame.area().bottom() {
                            Clear.render_ref(
                                Rect {
                                    x: frame.area().x,
                                    y: chat_bottom,
                                    width: frame.area().width,
                                    height: frame.area().bottom().saturating_sub(chat_bottom),
                                },
                                frame.buffer,
                            );
                        }

                        if !session_area.is_empty() {
                            frame.render_widget_ref(&self.session_bar, session_area);
                        }

                        if self.panel_focus == PanelFocus::Chat
                            && let Some((x, y)) = self.chat_widget.cursor_pos(chat_area)
                        {
                            frame.set_cursor_position((x, y));
                        }
                    })?;
                    let transcript_scrolled =
                        !matches!(self.transcript_scroll, TranscriptScroll::ToBottom);
                    let selection_active = matches!(
                        (self.transcript_selection.anchor, self.transcript_selection.head),
                        (Some(a), Some(b)) if a != b
                    );
                    let scroll_position = if self.transcript_total_lines == 0 {
                        None
                    } else {
                        Some((
                            self.transcript_view_top.saturating_add(1),
                            self.transcript_total_lines,
                        ))
                    };
                    self.chat_widget.set_transcript_ui_state(
                        transcript_scrolled,
                        selection_active,
                        scroll_position,
                    );
                }
            }
        }
        Ok(true)
    }

    pub(crate) fn render_transcript_cells(
        &mut self,
        frame: &mut Frame,
        cells: &[Arc<dyn HistoryCell>],
        chat_height: u16,
    ) -> u16 {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            self.transcript_scroll = TranscriptScroll::default();
            self.transcript_view_top = 0;
            self.transcript_total_lines = 0;
            return area.bottom().saturating_sub(chat_height);
        }

        let chat_height = chat_height.min(area.height);
        let max_transcript_height = area.height.saturating_sub(chat_height);
        if max_transcript_height == 0 {
            self.transcript_scroll = TranscriptScroll::default();
            self.transcript_view_top = 0;
            self.transcript_total_lines = 0;
            return area.y;
        }

        let transcript_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: max_transcript_height,
        };

        let (lines, line_meta) = Self::build_transcript_lines(cells, transcript_area.width);
        if lines.is_empty() {
            Clear.render_ref(transcript_area, frame.buffer);
            self.transcript_scroll = TranscriptScroll::default();
            self.transcript_view_top = 0;
            self.transcript_total_lines = 0;
            return area.y;
        }

        let wrapped = word_wrap_lines_borrowed(&lines, transcript_area.width.max(1) as usize);
        if wrapped.is_empty() {
            self.transcript_scroll = TranscriptScroll::default();
            self.transcript_view_top = 0;
            self.transcript_total_lines = 0;
            return area.y;
        }

        let is_user_cell: Vec<bool> = cells
            .iter()
            .map(|cell| cell.as_any().is::<UserHistoryCell>())
            .collect();
        let wrapped_line_meta =
            Self::build_wrapped_line_meta(&lines, &line_meta, transcript_area.width);
        debug_assert_eq!(wrapped.len(), wrapped_line_meta.len());
        let wrapped_is_user_row: Vec<bool> = wrapped_line_meta
            .iter()
            .map(TranscriptLineMeta::cell_index)
            .map(|cell_index| {
                cell_index
                    .and_then(|idx| is_user_cell.get(idx).copied())
                    .unwrap_or(false)
            })
            .collect();

        let total_lines = wrapped_line_meta.len();
        self.transcript_total_lines = total_lines;
        let max_visible = std::cmp::min(max_transcript_height as usize, total_lines);
        let max_start = total_lines.saturating_sub(max_visible);

        let (scroll_state, top_offset) = self
            .transcript_scroll
            .resolve_top(&wrapped_line_meta, max_start);
        self.transcript_scroll = scroll_state;
        self.transcript_view_top = top_offset;

        let transcript_visible_height = max_visible as u16;
        let chat_top = if total_lines <= max_transcript_height as usize {
            let gap = if transcript_visible_height == 0 { 0 } else { 1 };
            area.y
                .saturating_add(transcript_visible_height)
                .saturating_add(gap)
        } else {
            area.bottom().saturating_sub(chat_height)
        };

        let clear_height = chat_top.saturating_sub(area.y);
        if clear_height > 0 {
            Clear.render_ref(
                Rect {
                    x: area.x,
                    y: area.y,
                    width: area.width,
                    height: clear_height,
                },
                frame.buffer,
            );
        }

        let transcript_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: transcript_visible_height,
        };

        for (row_index, line_index) in (top_offset..total_lines).enumerate() {
            if row_index >= max_visible {
                break;
            }

            let y = transcript_area.y + row_index as u16;
            let row_area = Rect {
                x: transcript_area.x,
                y,
                width: transcript_area.width,
                height: 1,
            };

            if wrapped_is_user_row
                .get(line_index)
                .copied()
                .unwrap_or(false)
            {
                let base_style = crate::style::user_message_style();
                for x in row_area.x..row_area.right() {
                    let cell = &mut frame.buffer[(x, y)];
                    let style = cell.style().patch(base_style);
                    cell.set_style(style);
                }
            }

            wrapped[line_index].render_ref(row_area, frame.buffer);
        }

        self.apply_transcript_selection(transcript_area, frame.buffer);
        chat_top
    }

    fn handle_mouse_event(
        &mut self,
        tui: &mut tui::Tui,
        mouse_event: crossterm::event::MouseEvent,
    ) {
        use crossterm::event::MouseEventKind;

        if self.overlay.is_some() {
            return;
        }

        let viewport = tui.terminal.viewport_area;
        let width = viewport.width;
        let height = viewport.height;
        if width == 0 || height == 0 {
            return;
        }

        let session_height = 4u16.min(height);
        let available_for_chat = height.saturating_sub(session_height);
        let chat_height = self
            .chat_widget
            .desired_height(width)
            .min(available_for_chat);
        let reserved = chat_height.saturating_add(session_height);
        if reserved >= height {
            return;
        }

        let transcript_height = height.saturating_sub(reserved);
        if transcript_height == 0 {
            return;
        }

        let transcript_area = Rect {
            x: viewport.x,
            y: viewport.y,
            width,
            height: transcript_height,
        };
        let base_x = transcript_area.x.saturating_add(2);
        let max_x = transcript_area.right().saturating_sub(1);

        let mut clamped_x = mouse_event.column;
        let mut clamped_y = mouse_event.row;

        if clamped_y < transcript_area.y || clamped_y >= transcript_area.bottom() {
            clamped_y = transcript_area.y;
        }
        if clamped_x < base_x {
            clamped_x = base_x;
        }
        if clamped_x > max_x {
            clamped_x = max_x;
        }

        let streaming = self.chat_widget.is_task_running();

        match mouse_event.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_transcript(
                    tui,
                    -3,
                    transcript_area.height as usize,
                    transcript_area.width,
                );
            }
            MouseEventKind::ScrollDown => {
                self.scroll_transcript(
                    tui,
                    3,
                    transcript_area.height as usize,
                    transcript_area.width,
                );
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(point) = self.transcript_point_from_coordinates(
                    transcript_area,
                    base_x,
                    clamped_x,
                    clamped_y,
                ) {
                    self.transcript_selection.anchor = Some(point);
                    self.transcript_selection.head = Some(point);
                    tui.frame_requester().schedule_frame();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(anchor) = self.transcript_selection.anchor
                    && let Some(point) = self.transcript_point_from_coordinates(
                        transcript_area,
                        base_x,
                        clamped_x,
                        clamped_y,
                    )
                {
                    if streaming
                        && matches!(self.transcript_scroll, TranscriptScroll::ToBottom)
                        && point != anchor
                    {
                        self.lock_transcript_scroll_to_current_view(
                            transcript_area.height as usize,
                            transcript_area.width,
                        );
                    }
                    self.transcript_selection.head = Some(point);
                    tui.frame_requester().schedule_frame();
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.transcript_selection.anchor == self.transcript_selection.head {
                    self.transcript_selection = TranscriptSelection::default();
                    tui.frame_requester().schedule_frame();
                }
            }
            _ => {}
        }
    }

    fn scroll_transcript(
        &mut self,
        tui: &mut tui::Tui,
        delta_lines: i32,
        visible_lines: usize,
        width: u16,
    ) {
        if visible_lines == 0 {
            return;
        }

        let (lines, line_meta) = Self::build_transcript_lines(&self.transcript_cells, width);
        let wrapped_line_meta = Self::build_wrapped_line_meta(&lines, &line_meta, width);
        self.transcript_scroll =
            self.transcript_scroll
                .scrolled_by(delta_lines, &wrapped_line_meta, visible_lines);

        tui.frame_requester().schedule_frame();
    }

    fn lock_transcript_scroll_to_current_view(&mut self, visible_lines: usize, width: u16) {
        if self.transcript_cells.is_empty() || visible_lines == 0 || width == 0 {
            return;
        }

        let (lines, line_meta) = Self::build_transcript_lines(&self.transcript_cells, width);
        let wrapped_line_meta = Self::build_wrapped_line_meta(&lines, &line_meta, width);
        if wrapped_line_meta.is_empty() {
            return;
        }

        let total_lines = wrapped_line_meta.len();
        let max_visible = std::cmp::min(visible_lines, total_lines);
        if max_visible == 0 {
            return;
        }

        let max_start = total_lines.saturating_sub(max_visible);
        let top_offset = match self.transcript_scroll {
            TranscriptScroll::ToBottom => max_start,
            TranscriptScroll::Scrolled { .. } => return,
        };

        if let Some(scroll_state) = TranscriptScroll::anchor_for(&wrapped_line_meta, top_offset) {
            self.transcript_scroll = scroll_state;
        }
    }

    fn build_transcript_lines(
        cells: &[Arc<dyn HistoryCell>],
        width: u16,
    ) -> (Vec<Line<'static>>, Vec<TranscriptLineMeta>) {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut line_meta: Vec<TranscriptLineMeta> = Vec::new();
        let mut has_emitted_lines = false;

        for (cell_index, cell) in cells.iter().enumerate() {
            let cell_lines = cell.display_lines(width);
            if cell_lines.is_empty() {
                continue;
            }

            if !cell.is_stream_continuation() {
                if has_emitted_lines {
                    lines.push(Line::from(""));
                    line_meta.push(TranscriptLineMeta::Spacer);
                } else {
                    has_emitted_lines = true;
                }
            }

            for (line_in_cell, line) in cell_lines.into_iter().enumerate() {
                line_meta.push(TranscriptLineMeta::CellLine {
                    cell_index,
                    line_in_cell,
                });
                lines.push(line);
            }
        }

        (lines, line_meta)
    }

    fn copy_transcript_selection(&mut self, tui: &tui::Tui) {
        let (anchor, head) = match (
            self.transcript_selection.anchor,
            self.transcript_selection.head,
        ) {
            (Some(a), Some(h)) if a != h => (a, h),
            _ => return,
        };

        let viewport = tui.terminal.viewport_area;
        let width = viewport.width;
        let height = viewport.height;
        if width == 0 || height == 0 {
            return;
        }

        let session_height = 4u16.min(height);
        let available_for_chat = height.saturating_sub(session_height);
        let chat_height = self
            .chat_widget
            .desired_height(width)
            .min(available_for_chat);
        let reserved = chat_height.saturating_add(session_height);
        if reserved >= height {
            return;
        }

        let transcript_height = height.saturating_sub(reserved);
        if transcript_height == 0 {
            return;
        }

        let transcript_area = Rect {
            x: viewport.x,
            y: viewport.y,
            width,
            height: transcript_height,
        };

        let cells = self.transcript_cells.clone();
        let (lines, _) = Self::build_transcript_lines(&cells, transcript_area.width);
        if lines.is_empty() {
            return;
        }

        let wrapped = word_wrap_lines_borrowed(&lines, transcript_area.width.max(1) as usize);
        let total_lines = wrapped.len();
        if total_lines == 0 {
            return;
        }

        let max_visible = transcript_area.height as usize;
        let visible_start = self
            .transcript_view_top
            .min(total_lines.saturating_sub(max_visible));
        let visible_end = std::cmp::min(visible_start + max_visible, total_lines);

        let mut buf = Buffer::empty(transcript_area);
        Clear.render_ref(transcript_area, &mut buf);

        for (row_index, line_index) in (visible_start..visible_end).enumerate() {
            let row_area = Rect {
                x: transcript_area.x,
                y: transcript_area.y + row_index as u16,
                width: transcript_area.width,
                height: 1,
            };
            wrapped[line_index].render_ref(row_area, &mut buf);
        }

        let base_x = transcript_area.x.saturating_add(2);
        let max_x = transcript_area.right().saturating_sub(1);

        let mut start = anchor;
        let mut end = head;
        if (end.line_index < start.line_index)
            || (end.line_index == start.line_index && end.column < start.column)
        {
            std::mem::swap(&mut start, &mut end);
        }

        let mut lines_out: Vec<String> = Vec::new();

        for (row_index, line_index) in (visible_start..visible_end).enumerate() {
            if line_index < start.line_index || line_index > end.line_index {
                continue;
            }

            let y = transcript_area.y + row_index as u16;

            let line_start_col = if line_index == start.line_index {
                start.column
            } else {
                0
            };
            let line_end_col = if line_index == end.line_index {
                end.column
            } else {
                max_x.saturating_sub(base_x)
            };

            let row_sel_start = base_x.saturating_add(line_start_col);
            let row_sel_end = base_x.saturating_add(line_end_col).min(max_x);

            if row_sel_start > row_sel_end {
                continue;
            }

            let mut first_text_x = None;
            let mut last_text_x = None;
            for x in base_x..=max_x {
                let cell = &buf[(x, y)];
                if cell.symbol() != " " {
                    if first_text_x.is_none() {
                        first_text_x = Some(x);
                    }
                    last_text_x = Some(x);
                }
            }

            let (text_start, text_end) = match (first_text_x, last_text_x) {
                (Some(_), Some(e)) => (base_x, e),
                _ => {
                    lines_out.push(String::new());
                    continue;
                }
            };

            let from_x = row_sel_start.max(text_start);
            let to_x = row_sel_end.min(text_end);
            if from_x > to_x {
                continue;
            }

            let mut line_text = String::new();
            for x in from_x..=to_x {
                let cell = &buf[(x, y)];
                let symbol = cell.symbol();
                if !symbol.is_empty() {
                    line_text.push_str(symbol);
                }
            }

            lines_out.push(line_text);
        }

        if lines_out.is_empty() {
            return;
        }

        let text = lines_out.join("\n");
        if let Err(err) = clipboard_copy::copy_text(text) {
            tracing::error!(error = %err, "failed to copy selection to clipboard");
        }
    }

    fn transcript_point_from_coordinates(
        &self,
        transcript_area: Rect,
        base_x: u16,
        x: u16,
        y: u16,
    ) -> Option<TranscriptSelectionPoint> {
        if self.transcript_total_lines == 0 {
            return None;
        }

        let mut row_index = y.saturating_sub(transcript_area.y);
        if row_index >= transcript_area.height {
            if transcript_area.height == 0 {
                return None;
            }
            row_index = transcript_area.height.saturating_sub(1);
        }

        let max_line = self.transcript_total_lines.saturating_sub(1);
        let line_index = self
            .transcript_view_top
            .saturating_add(usize::from(row_index))
            .min(max_line);
        let column = x.saturating_sub(base_x);

        Some(TranscriptSelectionPoint { line_index, column })
    }

    async fn handle_event(&mut self, tui: &mut tui::Tui, event: AppEvent) -> Result<bool> {
        match event {
            AppEvent::UpdateSessionStatus {
                session_id: _,
                status: _,
            } => {
                // 已废弃：状态现在从 ChatWidget 实时读取
            }
            AppEvent::UpdateCurrentSessionStatus { status: _ } => {
                // 已废弃：状态现在从 ChatWidget 实时读取
            }
            AppEvent::SaveSessionAlias { session_id, alias } => {
                // Save alias in SessionBar
                self.session_bar.set_session_alias(session_id, alias);
                // Refresh session list to display updated alias
                self.session_bar.refresh_sessions();
                tui.frame_requester().schedule_frame();
            }
            AppEvent::NewSession => {
                let summary = session_summary(
                    self.chat_widget.token_usage(),
                    self.chat_widget.conversation_id(),
                );
                self.render_transcript_once(tui);
                self.transcript_cells.clear();
                self.deferred_history_lines.clear();
                self.has_emitted_history_lines = false;
                self.transcript_scroll = TranscriptScroll::default();
                self.transcript_selection = TranscriptSelection::default();
                self.transcript_view_top = 0;
                self.transcript_total_lines = 0;
                self.reset_backtrack_state();
                self.shutdown_current_conversation().await;
                let model = self
                    .models_manager
                    .get_model(&self.config.model, &self.config)
                    .await;
                let model_family = self
                    .models_manager
                    .construct_model_family(model.as_str(), &self.config)
                    .await;
                let init = crate::chatwidget::ChatWidgetInit {
                    config: self.config.clone(),
                    frame_requester: tui.frame_requester(),
                    app_event_tx: self.app_event_tx.clone(),
                    initial_prompt: None,
                    initial_images: Vec::new(),
                    enhanced_keys_supported: self.enhanced_keys_supported,
                    auth_manager: self.auth_manager.clone(),
                    models_manager: self.models_manager.clone(),
                    feedback: self.feedback.clone(),
                    is_first_run: false,
                    model_family: model_family.clone(),
                };
                self.chat_widget = ChatWidget::new(init, self.server.clone());
                self.current_model = model_family.get_model_slug().to_string();
                if let Some(summary) = summary {
                    let mut lines: Vec<Line<'static>> = vec![summary.usage_line.clone().into()];
                    if let Some(command) = summary.resume_command {
                        let spans = vec!["To continue this session, run ".into(), command.cyan()];
                        lines.push(spans.into());
                    }
                    self.chat_widget.add_plain_history_lines(lines);
                }

                // Switch focus to new chat
                self.panel_focus = PanelFocus::Chat;
                self.session_bar.set_focus(false);

                // Refresh session list
                self.session_bar.refresh_sessions();

                tui.frame_requester().schedule_frame();
            }
            AppEvent::OpenResumePicker => {
                self.open_or_refresh_session_picker(tui);
            }
            AppEvent::ResumeSession(path) => {
                if let Err(err) = self.resume_session_from_rollout(tui, path.clone()).await {
                    self.chat_widget.add_error_message(format!(
                        "Failed to resume session from {}: {err}",
                        path.display()
                    ));
                }
            }
            AppEvent::DelegateUpdate(update) => {
                self.handle_delegate_update(update);
            }
            AppEvent::TumixRunRequested {
                run_id,
                session_id,
                user_prompt,
                display_prompt,
            } => {
                self.start_tumix_run(run_id, session_id, user_prompt, display_prompt)
                    .await?;
            }
            AppEvent::InsertHistoryCell(cell) => {
                let cell: Arc<dyn HistoryCell> = cell.into();
                if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                    t.insert_cell(cell.clone());
                    tui.frame_requester().schedule_frame();
                }
                self.transcript_cells.push(cell.clone());
                let mut display = cell.display_lines(tui.terminal.last_known_screen_size.width);
                if !display.is_empty() {
                    // Only insert a separating blank line for new cells that are not
                    // part of an ongoing stream. Streaming continuations should not
                    // accrue extra blank lines between chunks.
                    if !cell.is_stream_continuation() {
                        if self.has_emitted_history_lines {
                            display.insert(0, Line::from(""));
                        } else {
                            self.has_emitted_history_lines = true;
                        }
                    }
                    if self.overlay.is_some() {
                        self.deferred_history_lines.extend(display);
                    }
                }
            }
            AppEvent::StartCommitAnimation => {
                if self
                    .commit_anim_running
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    let tx = self.app_event_tx.clone();
                    let running = self.commit_anim_running.clone();
                    thread::spawn(move || {
                        while running.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_millis(50));
                            tx.send(AppEvent::CommitTick);
                        }
                    });
                }
            }
            AppEvent::StopCommitAnimation => {
                self.commit_anim_running.store(false, Ordering::Release);
            }
            AppEvent::CommitTick => {
                self.chat_widget.on_commit_tick();
            }
            AppEvent::CodexEvent(event) => {
                // Backward-compat (should not be used anymore): assume it's for current conversation
                if self.suppress_shutdown_complete
                    && matches!(event.msg, EventMsg::ShutdownComplete)
                {
                    self.suppress_shutdown_complete = false;
                    return Ok(true);
                }
                self.chat_widget.handle_codex_event(event);
            }
            AppEvent::CodexEventFor {
                conversation_id,
                event,
            } => {
                let current = self.chat_widget.conversation_id().map(|id| id.to_string());
                // 仅当事件来源会话等于当前会话时才渲染；否则忽略（避免串线）
                if current.as_deref() == Some(conversation_id.as_str()) || current.is_none() {
                    self.chat_widget.handle_codex_event(event);
                } else {
                    // 非当前会话事件：忽略渲染。可在这里做轻量状态更新（如左侧标签），目前不做。
                }
            }
            AppEvent::ConversationHistory(ev) => {
                self.on_conversation_history_for_backtrack(tui, ev).await?;
            }
            AppEvent::ExitRequest => {
                return Ok(false);
            }
            AppEvent::CodexOp(op) => self.chat_widget.submit_op(op),
            AppEvent::DiffResult(text) => {
                // Clear the in-progress state in the bottom pane
                self.chat_widget.on_diff_complete();
                // Enter alternate screen using TUI helper and build pager lines
                let _ = tui.enter_alt_screen();
                let pager_lines: Vec<ratatui::text::Line<'static>> = if text.trim().is_empty() {
                    vec!["No changes detected.".italic().into()]
                } else {
                    text.lines().map(ansi_escape_line).collect()
                };
                self.overlay = Some(Overlay::new_static_with_lines(
                    pager_lines,
                    "D I F F".to_string(),
                ));
                tui.frame_requester().schedule_frame();
            }
            AppEvent::StartFileSearch(query) => {
                if !query.is_empty() {
                    self.file_search.on_user_query(query);
                }
            }
            AppEvent::FileSearchResult { query, matches } => {
                self.chat_widget.apply_file_search_result(query, matches);
            }
            AppEvent::RateLimitSnapshotFetched(snapshot) => {
                self.chat_widget.on_rate_limit_snapshot(Some(snapshot));
            }
            AppEvent::UpdateReasoningEffort(effort) => {
                self.on_update_reasoning_effort(effort);
            }
            AppEvent::UpdateModel(model) => {
                let model_family = self
                    .models_manager
                    .construct_model_family(&model, &self.config)
                    .await;
                self.chat_widget.set_model(&model, model_family);
                self.config.model = Some(model.clone());
                self.current_model = model.clone();

                // Keep the in‑memory provider configuration aligned with the
                // selected model family. This mirrors the logic used inside
                // `SessionConfiguration::apply` so that the TUI's status
                // output and session logs reflect the effective provider.
                let current_provider_id = Some(self.config.model_provider_id.as_str());
                if let Some(provider_id) = self
                    .config
                    .preferred_model_provider_id_for_model(current_provider_id, &model)
                    && let Some(provider) = self.config.model_providers.get(&provider_id)
                {
                    self.config.model_provider_id = provider_id;
                    self.config.model_provider = provider.clone();
                }
            }
            AppEvent::OpenReasoningPopup { model } => {
                self.chat_widget.open_reasoning_popup(model);
            }
            AppEvent::OpenAllModelsPopup { models } => {
                self.chat_widget.open_all_models_popup(models);
            }
            AppEvent::OpenFullAccessConfirmation { preset } => {
                self.chat_widget.open_full_access_confirmation(preset);
            }
            AppEvent::OpenWorldWritableWarningConfirmation {
                preset,
                sample_paths,
                extra_count,
                failed_scan,
            } => {
                self.chat_widget.open_world_writable_warning_confirmation(
                    preset,
                    sample_paths,
                    extra_count,
                    failed_scan,
                );
            }
            AppEvent::OpenFeedbackNote {
                category,
                include_logs,
            } => {
                self.chat_widget.open_feedback_note(category, include_logs);
            }
            AppEvent::OpenFeedbackConsent { category } => {
                self.chat_widget.open_feedback_consent(category);
            }
            AppEvent::OpenWindowsSandboxEnablePrompt { preset } => {
                self.chat_widget.open_windows_sandbox_enable_prompt(preset);
            }
            AppEvent::EnableWindowsSandboxForAgentMode { preset } => {
                #[cfg(target_os = "windows")]
                {
                    let profile = self.active_profile.as_deref();
                    let feature_key = Feature::WindowsSandbox.key();
                    match ConfigEditsBuilder::new(&self.config.codex_home)
                        .with_profile(profile)
                        .set_feature_enabled(feature_key, true)
                        .apply()
                        .await
                    {
                        Ok(()) => {
                            self.config.set_windows_sandbox_globally(true);
                            self.chat_widget.clear_forced_auto_mode_downgrade();
                            if let Some((sample_paths, extra_count, failed_scan)) =
                                self.chat_widget.world_writable_warning_details()
                            {
                                self.app_event_tx.send(
                                    AppEvent::OpenWorldWritableWarningConfirmation {
                                        preset: Some(preset.clone()),
                                        sample_paths,
                                        extra_count,
                                        failed_scan,
                                    },
                                );
                            } else {
                                self.app_event_tx.send(AppEvent::CodexOp(
                                    Op::OverrideTurnContext {
                                        cwd: None,
                                        approval_policy: Some(preset.approval),
                                        sandbox_policy: Some(preset.sandbox.clone()),
                                        model: None,
                                        effort: None,
                                        summary: None,
                                    },
                                ));
                                self.app_event_tx
                                    .send(AppEvent::UpdateAskForApprovalPolicy(preset.approval));
                                self.app_event_tx
                                    .send(AppEvent::UpdateSandboxPolicy(preset.sandbox.clone()));
                                self.chat_widget.add_info_message(
                                    "Enabled experimental Windows sandbox.".to_string(),
                                    None,
                                );
                            }
                        }
                        Err(err) => {
                            tracing::error!(
                                error = %err,
                                "failed to enable Windows sandbox feature"
                            );
                            self.chat_widget.add_error_message(format!(
                                "Failed to enable the Windows sandbox feature: {err}"
                            ));
                        }
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = preset;
                }
            }
            AppEvent::PersistModelSelection { model, effort } => {
                let profile = self.active_profile.as_deref();
                match ConfigEditsBuilder::new(&self.config.codex_home)
                    .with_profile(profile)
                    .set_model(Some(model.as_str()), effort)
                    .apply()
                    .await
                {
                    Ok(()) => {
                        let reasoning_label = Self::reasoning_label(effort);
                        if let Some(profile) = profile {
                            self.chat_widget.add_info_message(
                                format!(
                                    "Model changed to {model} {reasoning_label} for {profile} profile"
                                ),
                                None,
                            );
                        } else {
                            self.chat_widget.add_info_message(
                                format!("Model changed to {model} {reasoning_label}"),
                                None,
                            );
                        }
                    }
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "failed to persist model selection"
                        );
                        if let Some(profile) = profile {
                            self.chat_widget.add_error_message(format!(
                                "Failed to save model for profile `{profile}`: {err}"
                            ));
                        } else {
                            self.chat_widget
                                .add_error_message(format!("Failed to save default model: {err}"));
                        }
                    }
                }
            }
            AppEvent::UpdateAskForApprovalPolicy(policy) => {
                self.chat_widget.set_approval_policy(policy);
            }
            AppEvent::UpdateSandboxPolicy(policy) => {
                #[cfg(target_os = "windows")]
                let policy_is_workspace_write_or_ro = matches!(
                    policy,
                    codex_core::protocol::SandboxPolicy::WorkspaceWrite { .. }
                        | codex_core::protocol::SandboxPolicy::ReadOnly
                );

                self.config.sandbox_policy = policy.clone();
                #[cfg(target_os = "windows")]
                if !matches!(policy, codex_core::protocol::SandboxPolicy::ReadOnly)
                    || codex_core::get_platform_sandbox().is_some()
                {
                    self.config.forced_auto_mode_downgraded_on_windows = false;
                }
                self.chat_widget.set_sandbox_policy(policy);

                // If sandbox policy becomes workspace-write or read-only, run the Windows world-writable scan.
                #[cfg(target_os = "windows")]
                {
                    // One-shot suppression if the user just confirmed continue.
                    if self.skip_world_writable_scan_once {
                        self.skip_world_writable_scan_once = false;
                        return Ok(true);
                    }

                    let should_check = codex_core::get_platform_sandbox().is_some()
                        && policy_is_workspace_write_or_ro
                        && !self.chat_widget.world_writable_warning_hidden();
                    if should_check {
                        let cwd = self.config.cwd.clone();
                        let env_map: std::collections::HashMap<String, String> =
                            std::env::vars().collect();
                        let tx = self.app_event_tx.clone();
                        let logs_base_dir = self.config.codex_home.clone();
                        let sandbox_policy = self.config.sandbox_policy.clone();
                        Self::spawn_world_writable_scan(
                            cwd,
                            env_map,
                            logs_base_dir,
                            sandbox_policy,
                            tx,
                        );
                    }
                }
            }
            AppEvent::SkipNextWorldWritableScan => {
                self.skip_world_writable_scan_once = true;
            }
            AppEvent::UpdateFullAccessWarningAcknowledged(ack) => {
                self.chat_widget.set_full_access_warning_acknowledged(ack);
            }
            AppEvent::UpdateWorldWritableWarningAcknowledged(ack) => {
                self.chat_widget
                    .set_world_writable_warning_acknowledged(ack);
            }
            AppEvent::UpdateRateLimitSwitchPromptHidden(hidden) => {
                self.chat_widget.set_rate_limit_switch_prompt_hidden(hidden);
            }
            AppEvent::PersistFullAccessWarningAcknowledged => {
                if let Err(err) = ConfigEditsBuilder::new(&self.config.codex_home)
                    .set_hide_full_access_warning(true)
                    .apply()
                    .await
                {
                    tracing::error!(
                        error = %err,
                        "failed to persist full access warning acknowledgement"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to save full access confirmation preference: {err}"
                    ));
                }
            }
            AppEvent::PersistWorldWritableWarningAcknowledged => {
                if let Err(err) = ConfigEditsBuilder::new(&self.config.codex_home)
                    .set_hide_world_writable_warning(true)
                    .apply()
                    .await
                {
                    tracing::error!(
                        error = %err,
                        "failed to persist world-writable warning acknowledgement"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to save Agent mode warning preference: {err}"
                    ));
                }
            }
            AppEvent::PersistRateLimitSwitchPromptHidden => {
                if let Err(err) = ConfigEditsBuilder::new(&self.config.codex_home)
                    .set_hide_rate_limit_model_nudge(true)
                    .apply()
                    .await
                {
                    tracing::error!(
                        error = %err,
                        "failed to persist rate limit switch prompt preference"
                    );
                    self.chat_widget.add_error_message(format!(
                        "Failed to save rate limit reminder preference: {err}"
                    ));
                }
            }
            AppEvent::PersistModelMigrationPromptAcknowledged { migration_config } => {
                if let Err(err) = ConfigEditsBuilder::new(&self.config.codex_home)
                    .set_hide_model_migration_prompt(&migration_config, true)
                    .apply()
                    .await
                {
                    tracing::error!(error = %err, "failed to persist model migration prompt acknowledgement");
                    self.chat_widget.add_error_message(format!(
                        "Failed to save model migration prompt preference: {err}"
                    ));
                }
            }
            AppEvent::OpenApprovalsPopup => {
                self.chat_widget.open_approvals_popup();
            }
            AppEvent::OpenDelegatePicker => {
                let sessions = self.delegate_orchestrator.active_sessions().await;
                let detached_runs: Vec<DetachedRunSummary> =
                    self.delegate_orchestrator.detached_runs().await;
                let mut picker_sessions = Vec::with_capacity(sessions.len());
                for summary in sessions {
                    let run_id = if summary.mode == DelegateSessionMode::Detached {
                        self.delegate_orchestrator
                            .parent_run_for_conversation(summary.conversation_id.as_str())
                            .await
                    } else {
                        None
                    };
                    picker_sessions
                        .push(crate::chatwidget::DelegatePickerSession { summary, run_id });
                }
                self.chat_widget.open_delegate_picker(
                    picker_sessions,
                    detached_runs,
                    self.active_delegate.as_deref(),
                );
            }
            AppEvent::EnterDelegateSession(conversation_id) => {
                if let Err(err) = self.activate_delegate_session(tui, conversation_id).await {
                    tracing::error!("failed to enter delegate session: {err}");
                    self.chat_widget
                        .add_error_message(format!("Failed to open delegate: {err}"));
                }
            }
            AppEvent::ExitDelegateSession => {
                if let Err(err) = self.return_to_primary(tui).await {
                    tracing::error!("failed to return to primary agent: {err}");
                    self.chat_widget
                        .add_error_message(format!("Failed to return to main agent: {err}"));
                }
            }
            AppEvent::DismissDetachedRun(run_id) => {
                match self
                    .delegate_orchestrator
                    .dismiss_detached_run(&run_id)
                    .await
                {
                    Ok(()) => self
                        .chat_widget
                        .add_info_message(format!("Dismissed detached run {run_id}"), None),
                    Err(err) => self.chat_widget.add_error_message(err),
                }
            }
            AppEvent::InsertUserTextMessage(text) => {
                self.chat_widget.submit_text_message(text);
            }
            AppEvent::OpenReviewBranchPicker(cwd) => {
                self.chat_widget.show_review_branch_picker(&cwd).await;
            }
            AppEvent::OpenReviewCommitPicker(cwd) => {
                self.chat_widget.show_review_commit_picker(&cwd).await;
            }
            AppEvent::OpenReviewCustomPrompt => {
                self.chat_widget.show_review_custom_prompt();
            }
            AppEvent::CxresumeIdleCheck => {
                if self
                    .cxresume_idle
                    .handle_idle_check(self.overlay.is_some(), &self.app_event_tx)
                {
                    let tx = self.app_event_tx.clone();
                    tokio::spawn(async move {
                        let result = tokio::task::spawn_blocking(
                            crate::cxresume_picker_widget::load_picker_state,
                        )
                        .await;
                        match result {
                            Ok(Ok(state)) => {
                                tx.send(AppEvent::CxresumePrewarmReady(state));
                            }
                            Ok(Err(err)) => {
                                tx.send(AppEvent::CxresumePrewarmFailed(err));
                            }
                            Err(join_err) => {
                                tx.send(AppEvent::CxresumePrewarmFailed(join_err.to_string()));
                            }
                        }
                    });
                }
            }
            AppEvent::CxresumePrewarmReady(state) => {
                tracing::debug!(
                    "cxresume prewarm completed with {} sessions",
                    state.sessions.len()
                );
                self.update_cxresume_cache(state.clone());
                self.cxresume_idle.job_complete(&self.app_event_tx, true);
                if let Some(Overlay::SessionPicker(picker)) = self.overlay.as_mut() {
                    picker.replace_state(state);
                    tui.frame_requester().schedule_frame();
                }
            }
            AppEvent::CxresumePrewarmFailed(err) => {
                tracing::debug!("cxresume prewarm failed: {}", err);
                self.cxresume_idle.job_complete(&self.app_event_tx, false);
            }
            AppEvent::FullScreenApprovalRequest(request) => match request {
                ApprovalRequest::ApplyPatch { cwd, changes, .. } => {
                    let _ = tui.enter_alt_screen();
                    let diff_summary = DiffSummary::new(changes, cwd);
                    self.overlay = Some(Overlay::new_static_with_renderables(
                        vec![diff_summary.into()],
                        "P A T C H".to_string(),
                    ));
                }
                ApprovalRequest::Exec { command, .. } => {
                    let _ = tui.enter_alt_screen();
                    let full_cmd = strip_bash_lc_and_escape(&command);
                    let full_cmd_lines = highlight_bash_to_lines(&full_cmd);
                    self.overlay = Some(Overlay::new_static_with_lines(
                        full_cmd_lines,
                        "E X E C".to_string(),
                    ));
                }
                ApprovalRequest::McpElicitation {
                    server_name,
                    message,
                    ..
                } => {
                    let _ = tui.enter_alt_screen();
                    let paragraph = Paragraph::new(vec![
                        Line::from(vec!["Server: ".into(), server_name.bold()]),
                        Line::from(""),
                        Line::from(message),
                    ])
                    .wrap(Wrap { trim: false });
                    self.overlay = Some(Overlay::new_static_with_renderables(
                        vec![Box::new(paragraph)],
                        "E L I C I T A T I O N".to_string(),
                    ));
                }
            },
        }
        Ok(true)
    }

    fn handle_delegate_update(&mut self, event: DelegateEvent) {
        match event {
            DelegateEvent::Started {
                run_id,
                agent_id,
                prompt,
                parent_run_id,
                mode,
                ..
            } => {
                let display = self.delegate_tree.insert(
                    run_id.clone(),
                    agent_id.clone(),
                    parent_run_id.clone(),
                );
                let claim_status = parent_run_id.is_none() && self.delegate_status_owner.is_none();
                if claim_status {
                    self.delegate_status_owner = Some(run_id.clone());
                    self.chat_widget
                        .set_delegate_status_owner(&run_id, &agent_id);
                }
                self.chat_widget.on_delegate_started(
                    &run_id,
                    &agent_id,
                    &prompt,
                    display.label,
                    claim_status,
                    mode,
                );
                // 刷新会话栏以反映运行中状态
                self.session_bar.refresh_sessions();
            }
            DelegateEvent::Delta { run_id, chunk, .. } => {
                self.chat_widget.on_delegate_delta(&run_id, &chunk);
            }
            DelegateEvent::Completed {
                run_id,
                agent_id,
                output,
                duration,
                mode,
            } => {
                let display = self.delegate_tree.display_for(&run_id, &agent_id);
                self.delegate_tree.remove(&run_id);
                if self.delegate_status_owner.as_deref() == Some(run_id.as_str()) {
                    self.delegate_status_owner = None;
                    if let Some((next_run_id, next_agent)) = self.delegate_tree.first_active_root()
                    {
                        self.delegate_status_owner = Some(next_run_id.clone());
                        self.chat_widget
                            .set_delegate_status_owner(&next_run_id, &next_agent);
                    } else {
                        self.chat_widget.clear_delegate_status_owner();
                    }
                }
                let streamed = self
                    .chat_widget
                    .on_delegate_completed(&run_id, &display.label);
                let hint = Some(format!(
                    "finished in {}",
                    Self::format_delegate_duration(duration)
                ));
                let response = if display.depth == 0 {
                    output.as_deref().filter(|_| !streamed)
                } else {
                    None
                };
                self.chat_widget
                    .add_delegate_completion(response, hint, &display.label);
                if mode == DelegateSessionMode::Detached {
                    self.chat_widget.notify_detached_completion(&display.label);
                    self.chat_widget.show_detached_completion_actions(
                        &agent_id,
                        &run_id,
                        output.as_deref(),
                    );
                }
                // 刷新会话栏以反映完成状态
                self.session_bar.refresh_sessions();
            }
            DelegateEvent::Failed {
                run_id,
                agent_id,
                error,
                mode,
            } => {
                let display = self.delegate_tree.display_for(&run_id, &agent_id);
                self.delegate_tree.remove(&run_id);
                if self.delegate_status_owner.as_deref() == Some(run_id.as_str()) {
                    self.delegate_status_owner = None;
                    if let Some((next_run_id, next_agent)) = self.delegate_tree.first_active_root()
                    {
                        self.delegate_status_owner = Some(next_run_id.clone());
                        self.chat_widget
                            .set_delegate_status_owner(&next_run_id, &next_agent);
                    } else {
                        self.chat_widget.clear_delegate_status_owner();
                    }
                }
                self.chat_widget
                    .on_delegate_failed(&run_id, &display.label, &error);
                if mode == DelegateSessionMode::Detached {
                    self.chat_widget
                        .notify_detached_failure(&display.label, &error);
                }
                // 刷新会话栏以反映失败/停滞状态
                self.session_bar.refresh_sessions();
            }
        }
    }

    async fn activate_delegate_session(
        &mut self,
        tui: &mut tui::Tui,
        conversation_id: String,
    ) -> Result<(), String> {
        if self.active_delegate.as_deref() == Some(conversation_id.as_str()) {
            return Ok(());
        }

        if self.active_delegate.is_some() {
            self.stash_active_delegate();
        }

        let state = if let Some(state) = self.delegate_sessions.remove(&conversation_id) {
            state
        } else {
            let session = self
                .delegate_orchestrator
                .enter_session(&conversation_id)
                .await
                .map_err(|err| format!("{err}"))?;
            let model = self
                .models_manager
                .get_model(&session.config.model, &session.config)
                .await;
            let model_family = self
                .models_manager
                .construct_model_family(model.as_str(), &session.config)
                .await;
            let init = ChatWidgetInit {
                config: session.config.clone(),
                frame_requester: tui.frame_requester(),
                app_event_tx: self.app_event_tx.clone(),
                initial_prompt: None,
                initial_images: Vec::new(),
                enhanced_keys_supported: self.enhanced_keys_supported,
                auth_manager: self.auth_manager.clone(),
                models_manager: self.models_manager.clone(),
                feedback: self.feedback.clone(),
                is_first_run: false,
                model_family,
            };
            let session_configured = expect_unique_session_configured(session.session_configured);
            let mut chat_widget = ChatWidget::new_from_existing(
                init,
                conversation_id.clone(),
                session.conversation,
                session_configured,
            );
            chat_widget.set_delegate_context(Some(session.summary.clone()));
            DelegateSessionState {
                summary: session.summary,
                chat_widget,
            }
        };

        let DelegateSessionState {
            summary,
            mut chat_widget,
        } = state;
        chat_widget.set_delegate_context(Some(summary.clone()));
        let mut previous = std::mem::replace(&mut self.chat_widget, chat_widget);
        previous.set_delegate_context(None);
        self.primary_chat_backup = Some(previous);
        self.active_delegate = Some(conversation_id.clone());
        self.active_delegate_summary = Some(summary.clone());
        self.chat_widget.set_delegate_context(Some(summary.clone()));
        self.delegate_orchestrator
            .touch_session(&conversation_id)
            .await;
        tui.frame_requester().schedule_frame();
        Ok(())
    }

    fn stash_active_delegate(&mut self) {
        if let Some(active_id) = self.active_delegate.take() {
            let mut summary = match self.active_delegate_summary.take() {
                Some(summary) => summary,
                None => return,
            };
            let Some(main_chat) = self.primary_chat_backup.take() else {
                self.active_delegate_summary = Some(summary);
                return;
            };
            summary.last_interacted_at = SystemTime::now();
            let mut delegate_chat = std::mem::replace(&mut self.chat_widget, main_chat);
            delegate_chat.set_delegate_context(Some(summary.clone()));
            self.chat_widget.set_delegate_context(None);
            self.delegate_sessions.insert(
                active_id,
                DelegateSessionState {
                    summary,
                    chat_widget: delegate_chat,
                },
            );
        }
    }

    async fn return_to_primary(&mut self, tui: &mut tui::Tui) -> Result<(), String> {
        if let Some(active_id) = self.active_delegate.take() {
            let Some(mut summary) = self.active_delegate_summary.take() else {
                return Err("delegate summary missing".to_string());
            };
            let capture = self.chat_widget.take_delegate_capture();
            let main_chat = self
                .primary_chat_backup
                .take()
                .ok_or_else(|| "primary conversation unavailable".to_string())?;
            summary.last_interacted_at = SystemTime::now();
            let mut delegate_chat = std::mem::replace(&mut self.chat_widget, main_chat);
            delegate_chat.set_delegate_context(Some(summary.clone()));
            self.chat_widget.set_delegate_context(None);
            self.delegate_sessions.insert(
                active_id.clone(),
                DelegateSessionState {
                    summary: summary.clone(),
                    chat_widget: delegate_chat,
                },
            );
            self.delegate_orchestrator.touch_session(&active_id).await;
            self.primary_chat_backup = None;
            self.active_delegate_summary = None;
            if let Some(capture) = capture {
                self.chat_widget.apply_delegate_summary(&summary, capture);
            }
            tui.frame_requester().schedule_frame();
        }
        Ok(())
    }

    fn format_delegate_duration(duration: Duration) -> String {
        if duration.as_secs() >= 60 {
            let mins = duration.as_secs() / 60;
            let secs = duration.as_secs() % 60;
            format!("{mins}m{secs:02}s")
        } else if duration.as_millis() >= 1000 {
            format!("{:.1}s", duration.as_secs_f32())
        } else {
            format!("{:.0}ms", duration.as_millis())
        }
    }

    async fn start_tumix_run(
        &mut self,
        run_id: String,
        session_id: String,
        user_prompt: Option<String>,
        display_prompt: String,
    ) -> Result<()> {
        let agent_id = AgentId::parse("tumix")
            .map_err(|e| color_eyre::eyre::eyre!("failed to parse agent id: {e}"))?;
        let agent_id_for_task = agent_id.clone();
        self.handle_delegate_update(DelegateEvent::Started {
            run_id: run_id.clone(),
            agent_id,
            prompt: display_prompt,
            started_at: SystemTime::now(),
            parent_run_id: None,
            mode: DelegateSessionMode::Standard,
        });

        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let agent_id = agent_id_for_task;
            let start_time = Instant::now();
            let progress_tx = tx.clone();
            let progress_run_id = run_id.clone();
            let progress_agent_id = agent_id.clone();
            let progress_cb: codex_tumix::ProgressCallback = Box::new(move |msg: String| {
                progress_tx.send(AppEvent::DelegateUpdate(DelegateEvent::Delta {
                    run_id: progress_run_id.clone(),
                    agent_id: progress_agent_id.clone(),
                    chunk: msg,
                }));
            });

            let result = codex_tumix::run_tumix(session_id, user_prompt, Some(progress_cb)).await;

            match result {
                Ok(round_result) => {
                    let duration = start_time.elapsed();
                    let summary = format_tumix_summary(&round_result);
                    tx.send(AppEvent::DelegateUpdate(DelegateEvent::Completed {
                        run_id,
                        agent_id,
                        output: Some(summary),
                        duration,
                        mode: DelegateSessionMode::Standard,
                    }));
                }
                Err(err) => {
                    tx.send(AppEvent::DelegateUpdate(DelegateEvent::Failed {
                        run_id,
                        agent_id,
                        error: format!("TUMIX失败：{err}"),
                        mode: DelegateSessionMode::Standard,
                    }));
                }
            }
        });

        Ok(())
    }

    fn reasoning_label(reasoning_effort: Option<ReasoningEffortConfig>) -> &'static str {
        match reasoning_effort {
            Some(ReasoningEffortConfig::Minimal) => "minimal",
            Some(ReasoningEffortConfig::Low) => "low",
            Some(ReasoningEffortConfig::Medium) => "medium",
            Some(ReasoningEffortConfig::High) => "high",
            Some(ReasoningEffortConfig::XHigh) => "xhigh",
            None | Some(ReasoningEffortConfig::None) => "default",
        }
    }

    pub(crate) fn token_usage(&self) -> codex_core::protocol::TokenUsage {
        self.chat_widget.token_usage()
    }

    async fn resume_session_from_rollout(
        &mut self,
        tui: &mut tui::Tui,
        path: PathBuf,
    ) -> Result<()> {
        let summary = session_summary(
            self.chat_widget.token_usage(),
            self.chat_widget.conversation_id(),
        );
        let resumed = self
            .server
            .resume_conversation_from_rollout(
                self.config.clone(),
                path.clone(),
                self.auth_manager.clone(),
            )
            .await
            .wrap_err_with(|| format!("Failed to resume session from {}", path.display()))?;

        self.render_transcript_once(tui);
        self.transcript_cells.clear();
        self.deferred_history_lines.clear();
        self.has_emitted_history_lines = false;
        self.transcript_scroll = TranscriptScroll::default();
        self.transcript_selection = TranscriptSelection::default();
        self.transcript_view_top = 0;
        self.transcript_total_lines = 0;
        self.reset_backtrack_state();

        let model = self
            .models_manager
            .get_model(&self.config.model, &self.config)
            .await;
        let model_family = self
            .models_manager
            .construct_model_family(model.as_str(), &self.config)
            .await;
        let init = crate::chatwidget::ChatWidgetInit {
            config: self.config.clone(),
            frame_requester: tui.frame_requester(),
            app_event_tx: self.app_event_tx.clone(),
            initial_prompt: None,
            initial_images: Vec::new(),
            enhanced_keys_supported: self.enhanced_keys_supported,
            auth_manager: self.auth_manager.clone(),
            models_manager: self.models_manager.clone(),
            feedback: self.feedback.clone(),
            is_first_run: false,
            model_family: model_family.clone(),
        };

        self.shutdown_current_conversation().await;

        self.chat_widget = ChatWidget::new_from_existing(
            init,
            resumed.conversation_id.to_string(),
            resumed.conversation,
            resumed.session_configured,
        );
        self.current_model = model_family.get_model_slug().to_string();

        if let Some(summary) = summary {
            let mut lines: Vec<Line<'static>> = vec![summary.usage_line.clone().into()];
            if let Some(command) = summary.resume_command {
                let spans = vec!["To continue this session, run ".into(), command.cyan()];
                lines.push(spans.into());
            }
            self.chat_widget.add_plain_history_lines(lines);
        }

        // Switch focus to chat panel after loading session
        self.panel_focus = PanelFocus::Chat;
        self.session_bar.set_focus(false);

        // Refresh session list to update selection state
        self.session_bar.refresh_sessions();

        tui.frame_requester().schedule_frame();
        Ok(())
    }

    fn on_update_reasoning_effort(&mut self, effort: Option<ReasoningEffortConfig>) {
        self.chat_widget.set_reasoning_effort(effort);
        self.config.model_reasoning_effort = effort;
    }

    async fn handle_key_event(&mut self, tui: &mut tui::Tui, key_event: KeyEvent) {
        match key_event {
            // F1 Toggle Bar disabled per product decision
            KeyEvent {
                code: KeyCode::Char('t'),
                modifiers: crossterm::event::KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                ..
            } => {
                // Enter alternate screen and set viewport to full size.
                let _ = tui.enter_alt_screen();
                self.overlay = Some(Overlay::new_transcript(self.transcript_cells.clone()));
                tui.frame_requester().schedule_frame();
            }
            // Ctrl+P - Quick session search/picker (and switch focus to Sessions)
            KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: crossterm::event::KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                ..
            } => {
                // Focus sessions (bar is always visible now)
                self.panel_focus = PanelFocus::Sessions;
                self.session_bar.set_focus(true);
                let current_id = self.chat_widget.conversation_id().map(|id| id.to_string());
                self.session_bar
                    .reset_selection_for_focus(current_id.as_deref());
                tui.frame_requester().schedule_frame();
            }
            KeyEvent {
                code: KeyCode::Char('g'),
                modifiers: crossterm::event::KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                ..
            } => {
                // Show git graph for current directory
                match crate::git_graph_widget::create_git_graph_overlay(".") {
                    Ok(overlay) => {
                        let _ = tui.enter_alt_screen();
                        self.overlay = Some(overlay);
                        tui.frame_requester().schedule_frame();
                    }
                    Err(err) => {
                        // Show error message to user via overlay
                        let error_lines = vec![
                            "Failed to generate git graph:".red().into(),
                            Line::from(""),
                            err.clone().dim().into(),
                            Line::from(""),
                            "Make sure you are in a git repository.".italic().into(),
                        ];
                        let _ = tui.enter_alt_screen();
                        self.overlay = Some(Overlay::new_static_with_title(
                            error_lines,
                            "G I T   G R A P H   E R R O R".to_string(),
                        ));
                        tui.frame_requester().schedule_frame();
                        tracing::warn!("Failed to create git graph: {}", err);
                    }
                }
            }
            KeyEvent {
                code: KeyCode::Char('x' | 'q'),
                modifiers: crossterm::event::KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                ..
            } => {
                self.open_or_refresh_session_picker(tui);
            }
            // Esc primes/advances backtracking only in normal (not working) mode
            // with the composer focused and empty. In any other state, forward
            // Esc so the active UI (e.g. status indicator, modals, popups)
            // handles it.
            KeyEvent {
                code: KeyCode::Esc,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                // If in session tab mode, Esc exits selection without changing conversation
                if self.panel_focus == PanelFocus::Sessions {
                    self.panel_focus = PanelFocus::Chat;
                    self.session_bar.set_focus(false);
                    tui.frame_requester().schedule_frame();
                    return;
                }
                if self.chat_widget.is_normal_backtrack_mode()
                    && self.chat_widget.composer_is_empty()
                {
                    self.handle_backtrack_esc_key(tui);
                } else {
                    self.chat_widget.handle_key_event(key_event);
                }
            }
            KeyEvent {
                code: KeyCode::Char('y'),
                modifiers: crossterm::event::KeyModifiers::CONTROL,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } if self.panel_focus != PanelFocus::Sessions => {
                self.copy_transcript_selection(tui);
            }
            KeyEvent {
                code: KeyCode::PageUp,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } if self.panel_focus != PanelFocus::Sessions => {
                let viewport = tui.terminal.viewport_area;
                let width = viewport.width;
                let height = viewport.height;
                if width > 0 && height > 0 {
                    let session_height = 4u16.min(height);
                    let available_for_chat = height.saturating_sub(session_height);
                    let chat_height = self
                        .chat_widget
                        .desired_height(width)
                        .min(available_for_chat);
                    let reserved = chat_height.saturating_add(session_height);
                    if reserved < height {
                        let transcript_height = height.saturating_sub(reserved);
                        if transcript_height > 0 {
                            let delta = -i32::from(transcript_height);
                            self.scroll_transcript(
                                tui,
                                delta,
                                usize::from(transcript_height),
                                width,
                            );
                        }
                    }
                }
            }
            KeyEvent {
                code: KeyCode::PageDown,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } if self.panel_focus != PanelFocus::Sessions => {
                let viewport = tui.terminal.viewport_area;
                let width = viewport.width;
                let height = viewport.height;
                if width > 0 && height > 0 {
                    let session_height = 4u16.min(height);
                    let available_for_chat = height.saturating_sub(session_height);
                    let chat_height = self
                        .chat_widget
                        .desired_height(width)
                        .min(available_for_chat);
                    let reserved = chat_height.saturating_add(session_height);
                    if reserved < height {
                        let transcript_height = height.saturating_sub(reserved);
                        if transcript_height > 0 {
                            let delta = i32::from(transcript_height);
                            self.scroll_transcript(
                                tui,
                                delta,
                                usize::from(transcript_height),
                                width,
                            );
                        }
                    }
                }
            }
            KeyEvent {
                code: KeyCode::Home,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } if self.panel_focus != PanelFocus::Sessions => {
                if !self.transcript_cells.is_empty() {
                    self.transcript_scroll = TranscriptScroll::Scrolled {
                        cell_index: 0,
                        line_in_cell: 0,
                    };
                    tui.frame_requester().schedule_frame();
                }
            }
            KeyEvent {
                code: KeyCode::End,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } if self.panel_focus != PanelFocus::Sessions => {
                self.transcript_scroll = TranscriptScroll::ToBottom;
                tui.frame_requester().schedule_frame();
            }
            // Ctrl+C exits session tab mode (if active) without committing selection
            KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: crossterm::event::KeyModifiers::CONTROL,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if self.panel_focus == PanelFocus::Sessions {
                    self.panel_focus = PanelFocus::Chat;
                    self.session_bar.set_focus(false);
                    tui.frame_requester().schedule_frame();
                } else {
                    // Forward to chat (e.g., cancel in composer or ignore)
                    self.chat_widget.handle_key_event(key_event);
                }
            }
            // Enter confirms backtrack when primed + count > 0. Otherwise pass to widget.
            KeyEvent {
                code: KeyCode::Enter,
                kind: KeyEventKind::Press,
                ..
            } if self.backtrack.primed
                && self.backtrack.nth_user_message != usize::MAX
                && self.chat_widget.composer_is_empty() =>
            {
                // Delegate to helper for clarity; preserves behavior.
                self.confirm_backtrack_from_main();
            }
            KeyEvent {
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                // Any non-Esc key press should cancel a primed backtrack.
                // This avoids stale "Esc-primed" state after the user starts typing
                // (even if they later backspace to empty).
                if key_event.code != KeyCode::Esc && self.backtrack.primed {
                    self.reset_backtrack_state();
                }

                // Route key events based on focus
                match self.panel_focus {
                    PanelFocus::Sessions => {
                        // Handle session bar navigation (horizontal)
                        match key_event.code {
                            KeyCode::Left | KeyCode::Char('h') => {
                                self.session_bar.select_previous();
                                tui.frame_requester().schedule_frame();
                            }
                            KeyCode::Right | KeyCode::Char('l') => {
                                self.session_bar.select_next();
                                tui.frame_requester().schedule_frame();
                            }
                            KeyCode::Char('n') => {
                                // 快速新建会话
                                self.app_event_tx.send(AppEvent::NewSession);
                            }
                            KeyCode::Enter => {
                                // Enter on New vs History session
                                if self.session_bar.selected_is_new() {
                                    self.app_event_tx.send(AppEvent::NewSession);
                                } else if let Some(session) = self.session_bar.selected_session() {
                                    self.app_event_tx
                                        .send(AppEvent::ResumeSession(session.path.clone()));
                                }
                            }
                            KeyCode::Char('r') => {
                                // Rename selected session (edit alias) - only works on existing sessions
                                if !self.session_bar.selected_is_new()
                                    && let Some(session) = self.session_bar.selected_session()
                                {
                                    let session_id = session.id.clone();
                                    let app_tx = self.app_event_tx.clone();

                                    // Show alias input for renaming
                                    self.chat_widget.show_session_alias_input_for_rename(
                                        session_id,
                                        Box::new(move |sid, alias| {
                                            app_tx.send(AppEvent::SaveSessionAlias {
                                                session_id: sid,
                                                alias,
                                            });
                                        }),
                                    );

                                    // Transfer focus to ChatWidget so the rename dialog can receive input
                                    self.panel_focus = PanelFocus::Chat;
                                    self.session_bar.set_focus(false);
                                    tui.frame_requester().schedule_frame();
                                }
                            }
                            KeyCode::Char('x') => {
                                // Delete selected history session rollout file (no confirmation)
                                if !self.session_bar.selected_is_new()
                                    && let Some(session) = self.session_bar.selected_session()
                                {
                                    // Clone values before mutable borrow
                                    let session_path = session.path.clone();
                                    let session_id = session.id.clone();

                                    // Remove the session file
                                    let _ = std::fs::remove_file(&session_path);
                                    // Remove the associated alias
                                    self.session_bar.remove_session_alias(&session_id);
                                    // Do NOT switch conversation; just refresh list
                                    self.session_bar.refresh_sessions();
                                    tui.frame_requester().schedule_frame();
                                }
                            }
                            // Exit sessions focus; Tab no longer toggles to avoid conflicts
                            KeyCode::Esc => {
                                // Return focus to chat
                                self.panel_focus = PanelFocus::Chat;
                                self.session_bar.set_focus(false);
                                tui.frame_requester().schedule_frame();
                            }
                            _ => {}
                        }
                    }
                    PanelFocus::Chat => {
                        self.chat_widget.handle_key_event(key_event);
                    }
                }
            }
            _ => {
                // Ignore Release key events.
            }
        };
    }

    #[cfg(target_os = "windows")]
    fn spawn_world_writable_scan(
        cwd: PathBuf,
        env_map: std::collections::HashMap<String, String>,
        logs_base_dir: PathBuf,
        sandbox_policy: codex_core::protocol::SandboxPolicy,
        tx: AppEventSender,
    ) {
        tokio::task::spawn_blocking(move || {
            let result = codex_windows_sandbox::apply_world_writable_scan_and_denies(
                &logs_base_dir,
                &cwd,
                &env_map,
                &sandbox_policy,
                Some(logs_base_dir.as_path()),
            );
            if result.is_err() {
                // Scan failed: warn without examples.
                tx.send(AppEvent::OpenWorldWritableWarningConfirmation {
                    preset: None,
                    sample_paths: Vec::new(),
                    extra_count: 0usize,
                    failed_scan: true,
                });
            }
        });
    }
}

struct DelegateSessionState {
    summary: DelegateSessionSummary,
    chat_widget: ChatWidget,
}

fn expect_unique_session_configured(
    session_configured: Arc<SessionConfiguredEvent>,
) -> SessionConfiguredEvent {
    Arc::unwrap_or_clone(session_configured)
}

struct CxresumeIdleLoader {
    idle_after: Duration,
    last_activity: Instant,
    job_in_flight: bool,
    cooldown_until: Option<Instant>,
    pending_check: Option<JoinHandle<()>>,
}

impl CxresumeIdleLoader {
    fn new(idle_after: Duration) -> Self {
        Self {
            idle_after,
            last_activity: Instant::now(),
            job_in_flight: false,
            cooldown_until: None,
            pending_check: None,
        }
    }

    fn on_user_activity(&mut self, tx: &AppEventSender) {
        self.last_activity = Instant::now();
        if !self.job_in_flight {
            self.schedule_after(tx, self.idle_after);
        }
    }

    fn handle_idle_check(&mut self, overlay_active: bool, tx: &AppEventSender) -> bool {
        if self.job_in_flight {
            return false;
        }
        if overlay_active {
            self.schedule_after(tx, self.idle_after);
            return false;
        }

        let now = Instant::now();
        if let Some(deadline) = self.cooldown_until
            && now < deadline
        {
            let remaining = deadline.saturating_duration_since(now);
            self.schedule_after(tx, remaining);
            return false;
        }

        let since_activity = now.saturating_duration_since(self.last_activity);
        if since_activity < self.idle_after {
            let remaining = self.idle_after - since_activity;
            self.schedule_after(tx, remaining);
            return false;
        }

        self.job_in_flight = true;
        self.cancel_pending();
        true
    }

    fn job_complete(&mut self, tx: &AppEventSender, success: bool) {
        self.job_in_flight = false;
        self.last_activity = Instant::now();
        let cooldown = if success {
            Duration::from_secs(300)
        } else {
            Duration::from_secs(60)
        };
        self.cooldown_until = Some(self.last_activity + cooldown);
        self.schedule_after(tx, cooldown);
    }

    fn cancel_pending(&mut self) {
        if let Some(handle) = self.pending_check.take() {
            handle.abort();
        }
    }

    fn schedule_after(&mut self, tx: &AppEventSender, delay: Duration) {
        if self.job_in_flight {
            return;
        }
        self.cancel_pending();
        let tx = tx.clone();
        self.pending_check = Some(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            tx.send(AppEvent::CxresumeIdleCheck);
        }));
    }

    fn trigger_immediate(&mut self, tx: &AppEventSender) {
        if self.job_in_flight {
            return;
        }
        self.last_activity = Instant::now()
            .checked_sub(self.idle_after)
            .unwrap_or_else(Instant::now);
        self.schedule_after(tx, Duration::ZERO);
    }
}

impl Drop for CxresumeIdleLoader {
    fn drop(&mut self) {
        self.cancel_pending();
    }
}

impl App {
    fn build_wrapped_line_meta(
        lines: &[Line<'static>],
        line_meta: &[TranscriptLineMeta],
        width: u16,
    ) -> Vec<TranscriptLineMeta> {
        let base_opts: RtOptions<'_> = RtOptions::new(width.max(1) as usize);
        let mut wrapped_meta: Vec<TranscriptLineMeta> = Vec::new();
        let mut first = true;
        let mut current_cell = None;
        let mut current_line_in_cell = 0usize;

        for (idx, line) in lines.iter().enumerate() {
            let opts = if first {
                base_opts.clone()
            } else {
                base_opts
                    .clone()
                    .initial_indent(base_opts.subsequent_indent.clone())
            };
            let seg_count = word_wrap_line(line, opts).len();
            let meta = line_meta
                .get(idx)
                .copied()
                .unwrap_or(TranscriptLineMeta::Spacer);

            match meta {
                TranscriptLineMeta::CellLine { cell_index, .. } => {
                    if current_cell != Some(cell_index) {
                        current_cell = Some(cell_index);
                        current_line_in_cell = 0;
                    }
                    for offset in 0..seg_count {
                        wrapped_meta.push(TranscriptLineMeta::CellLine {
                            cell_index,
                            line_in_cell: current_line_in_cell + offset,
                        });
                    }
                    current_line_in_cell += seg_count;
                }
                TranscriptLineMeta::Spacer => {
                    wrapped_meta.extend(std::iter::repeat_n(TranscriptLineMeta::Spacer, seg_count));
                    current_cell = None;
                }
            }
            first = false;
        }

        wrapped_meta
    }

    fn render_lines_to_ansi(
        lines: &[Line<'static>],
        line_meta: &[TranscriptLineMeta],
        is_user_cell: &[bool],
        width: u16,
    ) -> Vec<String> {
        lines
            .iter()
            .enumerate()
            .map(|(idx, line)| {
                let is_user_row = line_meta
                    .get(idx)
                    .and_then(TranscriptLineMeta::cell_index)
                    .map(|cell_index| is_user_cell.get(cell_index).copied().unwrap_or(false))
                    .unwrap_or(false);

                let mut merged_spans: Vec<ratatui::text::Span<'static>> = line
                    .spans
                    .iter()
                    .map(|span| ratatui::text::Span {
                        style: span.style.patch(line.style),
                        content: span.content.clone(),
                    })
                    .collect();

                if is_user_row && width > 0 {
                    let text: String = merged_spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect();
                    let text_width = UnicodeWidthStr::width(text.as_str());
                    let total_width = usize::from(width);
                    if text_width < total_width {
                        let pad_len = total_width.saturating_sub(text_width);
                        if pad_len > 0 {
                            let pad_style = crate::style::user_message_style();
                            merged_spans.push(ratatui::text::Span {
                                style: pad_style,
                                content: " ".repeat(pad_len).into(),
                            });
                        }
                    }
                }

                let mut buf: Vec<u8> = Vec::new();
                let _ = crate::insert_history::write_spans(&mut buf, merged_spans.iter());
                String::from_utf8(buf).unwrap_or_default()
            })
            .collect()
    }

    fn apply_transcript_selection(&self, area: Rect, buf: &mut Buffer) {
        let (anchor, head) = match (
            self.transcript_selection.anchor,
            self.transcript_selection.head,
        ) {
            (Some(a), Some(h)) => (a, h),
            _ => return,
        };

        if self.transcript_total_lines == 0 {
            return;
        }

        let base_x = area.x.saturating_add(2);
        let max_x = area.right().saturating_sub(1);

        let mut start = anchor;
        let mut end = head;
        if (end.line_index < start.line_index)
            || (end.line_index == start.line_index && end.column < start.column)
        {
            std::mem::swap(&mut start, &mut end);
        }

        let visible_start = self.transcript_view_top;
        let visible_end = self
            .transcript_view_top
            .saturating_add(area.height as usize)
            .min(self.transcript_total_lines);

        for (row_index, line_index) in (visible_start..visible_end).enumerate() {
            if line_index < start.line_index || line_index > end.line_index {
                continue;
            }

            let y = area.y + row_index as u16;

            let mut first_text_x = None;
            let mut last_text_x = None;
            for x in base_x..=max_x {
                let cell = &buf[(x, y)];
                if cell.symbol() != " " {
                    if first_text_x.is_none() {
                        first_text_x = Some(x);
                    }
                    last_text_x = Some(x);
                }
            }

            let (text_start, text_end) = match (first_text_x, last_text_x) {
                (Some(_), Some(e)) => (base_x, e),
                _ => continue,
            };

            let line_start_col = if line_index == start.line_index {
                start.column
            } else {
                0
            };
            let line_end_col = if line_index == end.line_index {
                end.column
            } else {
                max_x.saturating_sub(base_x)
            };

            let row_sel_start = base_x.saturating_add(line_start_col);
            let row_sel_end = base_x.saturating_add(line_end_col).min(max_x);

            if row_sel_start > row_sel_end {
                continue;
            }

            let from_x = row_sel_start.max(text_start);
            let to_x = row_sel_end.min(text_end);

            if from_x > to_x {
                continue;
            }

            for x in from_x..=to_x {
                let cell = &mut buf[(x, y)];
                let style = cell.style();
                cell.set_style(style.add_modifier(ratatui::style::Modifier::REVERSED));
            }
        }
    }
}

fn format_tumix_summary(result: &Round1Result) -> String {
    if result.agents.is_empty() {
        return "⚠️ TUMIX Round 1 完成，但没有任何 agent 返回结果。".to_string();
    }

    let branch_lines = result
        .agents
        .iter()
        .map(|agent| {
            let commit_short = agent.commit_hash.chars().take(8).collect::<String>();
            format!("  - {} (commit: {})", agent.branch, commit_short)
        })
        .collect::<Vec<_>>();

    format!(
        "✨ TUMIX Round 1 完成\n\
         📊 共执行 {} 个 agent\n\
         📁 详细日志与会话文件位于 `.tumix/`\n\
         🌳 生成分支：\n{}",
        result.agents.len(),
        branch_lines.join("\n")
    )
}

fn migration_prompt_allowed_auth_modes(migration_config_key: &str) -> Option<&'static [AuthMode]> {
    match migration_config_key {
        HIDE_GPT5_1_MIGRATION_PROMPT_CONFIG => Some(&GPT_5_1_MIGRATION_AUTH_MODES),
        HIDE_GPT_5_1_CODEX_MAX_MIGRATION_PROMPT_CONFIG => Some(&GPT_5_1_CODEX_MIGRATION_AUTH_MODES),
        _ => None,
    }
}

fn migration_prompt_allows_auth_mode(
    auth_mode: Option<AuthMode>,
    migration_config_key: &str,
) -> bool {
    if let Some(allowed_modes) = migration_prompt_allowed_auth_modes(migration_config_key) {
        match auth_mode {
            None => true,
            Some(mode) => allowed_modes.contains(&mode),
        }
    } else {
        auth_mode != Some(AuthMode::ApiKey)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_backtrack::BacktrackState;
    use crate::app_backtrack::user_count;
    use crate::chatwidget::tests::make_chatwidget_manual_with_sender;
    use crate::file_search::FileSearchManager;
    use crate::history_cell::AgentMessageCell;
    use crate::history_cell::HistoryCell;
    use crate::history_cell::UserHistoryCell;
    use crate::history_cell::new_session_info;
    use codex_core::AuthManager;
    use codex_core::CodexAuth;
    use codex_core::ConversationManager;
    use codex_core::protocol::AskForApproval;
    use codex_core::protocol::Event;
    use codex_core::protocol::EventMsg;
    use codex_core::protocol::SandboxPolicy;
    use codex_core::protocol::SessionConfiguredEvent;
    use codex_protocol::ConversationId;
    use pretty_assertions::assert_eq;
    use ratatui::prelude::Line;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn test_codex_home() -> PathBuf {
        let path = std::env::temp_dir().join("codex-tui-test-home");
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("create test codex home {}: {err}", path.display()));
        path
    }

    async fn make_test_app() -> App {
        let (chat_widget, app_event_tx, _rx, _op_rx) = make_chatwidget_manual_with_sender().await;
        let mut config = chat_widget.config_ref().clone();
        config.codex_home = test_codex_home();
        let current_model = chat_widget.get_model_family().get_model_slug().to_string();
        let server = Arc::new(ConversationManager::with_models_provider_and_home(
            CodexAuth::from_api_key("Test API Key"),
            config.model_provider.clone(),
            config.codex_home.clone(),
        ));
        let auth_manager = AuthManager::from_auth_for_testing_with_home(
            CodexAuth::from_api_key("Test API Key"),
            config.codex_home.clone(),
        );
        let models_manager = Arc::new(ModelsManager::new(auth_manager.clone()));
        let delegate_orchestrator = Arc::new(AgentOrchestrator::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            codex_common::CliConfigOverrides::default(),
            codex_core::config::ConfigOverrides::default(),
            Vec::new(),
            config.multi_agent.max_concurrent_delegates,
        ));
        let file_search = FileSearchManager::new(config.cwd.clone(), app_event_tx.clone());
        let session_bar = SessionBar::new(config.cwd.clone(), config.codex_home.clone());

        App {
            server,
            app_event_tx,
            chat_widget,
            auth_manager,
            models_manager,
            delegate_orchestrator,
            config,
            current_model,
            active_profile: None,
            file_search,
            transcript_cells: Vec::new(),
            transcript_scroll: TranscriptScroll::default(),
            transcript_selection: TranscriptSelection::default(),
            transcript_view_top: 0,
            transcript_total_lines: 0,
            overlay: None,
            deferred_history_lines: Vec::new(),
            has_emitted_history_lines: false,
            enhanced_keys_supported: false,
            commit_anim_running: Arc::new(AtomicBool::new(false)),
            backtrack: BacktrackState::default(),
            cxresume_cache: None,
            cxresume_idle: CxresumeIdleLoader::new(Duration::from_secs(2)),
            feedback: codex_feedback::CodexFeedback::new(),
            delegate_sessions: HashMap::new(),
            active_delegate: None,
            active_delegate_summary: None,
            primary_chat_backup: None,
            pending_update_action: None,
            delegate_tree: DelegateTree::default(),
            delegate_status_owner: None,
            suppress_shutdown_complete: false,
            skip_world_writable_scan_once: false,
            session_bar,
            panel_focus: PanelFocus::Chat,
            layout_mode: LayoutMode::Normal,
        }
    }

    async fn make_test_app_with_channels() -> (
        App,
        tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
        tokio::sync::mpsc::UnboundedReceiver<Op>,
    ) {
        let (chat_widget, app_event_tx, rx, op_rx) = make_chatwidget_manual_with_sender().await;
        let mut config = chat_widget.config_ref().clone();
        config.codex_home = test_codex_home();
        let current_model = chat_widget.get_model_family().get_model_slug().to_string();
        let server = Arc::new(ConversationManager::with_models_provider_and_home(
            CodexAuth::from_api_key("Test API Key"),
            config.model_provider.clone(),
            config.codex_home.clone(),
        ));
        let auth_manager = AuthManager::from_auth_for_testing_with_home(
            CodexAuth::from_api_key("Test API Key"),
            config.codex_home.clone(),
        );
        let models_manager = Arc::new(ModelsManager::new(auth_manager.clone()));
        let delegate_orchestrator = Arc::new(AgentOrchestrator::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            SessionSource::Cli,
            codex_common::CliConfigOverrides::default(),
            codex_core::config::ConfigOverrides::default(),
            Vec::new(),
            config.multi_agent.max_concurrent_delegates,
        ));
        let file_search = FileSearchManager::new(config.cwd.clone(), app_event_tx.clone());
        let session_bar = SessionBar::new(config.cwd.clone(), config.codex_home.clone());

        (
            App {
                server,
                app_event_tx,
                chat_widget,
                auth_manager,
                models_manager,
                delegate_orchestrator,
                config,
                current_model,
                active_profile: None,
                file_search,
                transcript_cells: Vec::new(),
                transcript_scroll: TranscriptScroll::default(),
                transcript_selection: TranscriptSelection::default(),
                transcript_view_top: 0,
                transcript_total_lines: 0,
                overlay: None,
                deferred_history_lines: Vec::new(),
                has_emitted_history_lines: false,
                enhanced_keys_supported: false,
                commit_anim_running: Arc::new(AtomicBool::new(false)),
                backtrack: BacktrackState::default(),
                cxresume_cache: None,
                cxresume_idle: CxresumeIdleLoader::new(Duration::from_secs(2)),
                feedback: codex_feedback::CodexFeedback::new(),
                delegate_sessions: HashMap::new(),
                active_delegate: None,
                active_delegate_summary: None,
                primary_chat_backup: None,
                pending_update_action: None,
                delegate_tree: DelegateTree::default(),
                delegate_status_owner: None,
                suppress_shutdown_complete: false,
                skip_world_writable_scan_once: false,
                session_bar,
                panel_focus: PanelFocus::Chat,
                layout_mode: LayoutMode::Normal,
            },
            rx,
            op_rx,
        )
    }

    fn all_model_presets() -> Vec<ModelPreset> {
        codex_core::openai_models::model_presets::all_model_presets().clone()
    }

    #[tokio::test]
    async fn model_migration_prompt_only_shows_for_deprecated_models() {
        assert!(should_show_model_migration_prompt(
            "gpt-5",
            "gpt-5.1",
            false,
            &all_model_presets()
        ));
        assert!(should_show_model_migration_prompt(
            "gpt-5-codex",
            "gpt-5.1-codex",
            false,
            &all_model_presets()
        ));
        assert!(should_show_model_migration_prompt(
            "gpt-5-codex-mini",
            "gpt-5.1-codex-mini",
            false,
            &all_model_presets()
        ));
        assert!(should_show_model_migration_prompt(
            "gpt-5.1-codex",
            "gpt-5.1-codex-max",
            false,
            &all_model_presets()
        ));
        assert!(!should_show_model_migration_prompt(
            "gpt-5.1-codex",
            "gpt-5.1-codex",
            false,
            &all_model_presets()
        ));
    }

    #[tokio::test]
    async fn model_migration_prompt_respects_hide_flag_and_self_target() {
        assert!(!should_show_model_migration_prompt(
            "gpt-5",
            "gpt-5.1",
            true,
            &all_model_presets()
        ));
        assert!(!should_show_model_migration_prompt(
            "gpt-5.1",
            "gpt-5.1",
            false,
            &all_model_presets()
        ));
    }

    #[tokio::test]
    async fn update_reasoning_effort_updates_config() {
        let mut app = make_test_app().await;
        app.config.model_reasoning_effort = Some(ReasoningEffortConfig::Medium);
        app.chat_widget
            .set_reasoning_effort(Some(ReasoningEffortConfig::Medium));

        app.on_update_reasoning_effort(Some(ReasoningEffortConfig::High));

        assert_eq!(
            app.config.model_reasoning_effort,
            Some(ReasoningEffortConfig::High)
        );
        assert_eq!(
            app.chat_widget.config_ref().model_reasoning_effort,
            Some(ReasoningEffortConfig::High)
        );
    }

    #[tokio::test]
    async fn backtrack_selection_with_duplicate_history_targets_unique_turn() {
        let mut app = make_test_app().await;

        let user_cell = |text: &str| -> Arc<dyn HistoryCell> {
            Arc::new(UserHistoryCell {
                message: text.to_string(),
            }) as Arc<dyn HistoryCell>
        };
        let agent_cell = |text: &str| -> Arc<dyn HistoryCell> {
            Arc::new(AgentMessageCell::new(
                vec![Line::from(text.to_string())],
                true,
            )) as Arc<dyn HistoryCell>
        };

        let make_header = |is_first| {
            let event = SessionConfiguredEvent {
                session_id: ConversationId::new(),
                model: "gpt-test".to_string(),
                model_provider_id: "test-provider".to_string(),
                approval_policy: AskForApproval::Never,
                sandbox_policy: SandboxPolicy::ReadOnly,
                cwd: PathBuf::from("/home/user/project"),
                reasoning_effort: None,
                history_log_id: 0,
                history_entry_count: 0,
                initial_messages: None,
                skill_load_outcome: None,
                rollout_path: PathBuf::new(),
            };
            Arc::new(new_session_info(
                app.chat_widget.config_ref(),
                app.current_model.as_str(),
                event,
                is_first,
            )) as Arc<dyn HistoryCell>
        };

        // Simulate the transcript after trimming for a fork, replaying history, and
        // appending the edited turn. The session header separates the retained history
        // from the forked conversation's replayed turns.
        app.transcript_cells = vec![
            make_header(true),
            user_cell("first question"),
            agent_cell("answer first"),
            user_cell("follow-up"),
            agent_cell("answer follow-up"),
            make_header(false),
            user_cell("first question"),
            agent_cell("answer first"),
            user_cell("follow-up (edited)"),
            agent_cell("answer edited"),
        ];

        assert_eq!(user_count(&app.transcript_cells), 2);

        app.backtrack.base_id = Some(ConversationId::new());
        app.backtrack.primed = true;
        app.backtrack.nth_user_message = user_count(&app.transcript_cells).saturating_sub(1);

        app.confirm_backtrack_from_main();

        let (_, nth, prefill) = app.backtrack.pending.clone().expect("pending backtrack");
        assert_eq!(nth, 1);
        assert_eq!(prefill, "follow-up (edited)");
    }

    #[tokio::test]
    async fn transcript_selection_moves_with_scroll() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let mut app = make_test_app().await;
        app.transcript_total_lines = 3;

        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 2,
        };

        // Anchor selection to logical line 1, columns 2..4.
        app.transcript_selection = TranscriptSelection {
            anchor: Some(TranscriptSelectionPoint {
                line_index: 1,
                column: 2,
            }),
            head: Some(TranscriptSelectionPoint {
                line_index: 1,
                column: 4,
            }),
        };

        // First render: top of view is line 0, so line 1 maps to the second row.
        app.transcript_view_top = 0;
        let mut buf = Buffer::empty(area);
        for x in 2..area.width {
            buf[(x, 0)].set_symbol("A");
            buf[(x, 1)].set_symbol("B");
        }

        app.apply_transcript_selection(area, &mut buf);

        // No selection should be applied to the first row when the view is anchored at the top.
        for x in 0..area.width {
            let cell = &buf[(x, 0)];
            assert!(cell.style().add_modifier.is_empty());
        }

        // After scrolling down by one line, the same logical line should now be
        // rendered on the first row, and the highlight should move with it.
        app.transcript_view_top = 1;
        let mut buf_scrolled = Buffer::empty(area);
        for x in 2..area.width {
            buf_scrolled[(x, 0)].set_symbol("B");
            buf_scrolled[(x, 1)].set_symbol("C");
        }

        app.apply_transcript_selection(area, &mut buf_scrolled);

        // After scrolling, the selection should now be applied on the first row rather than the
        // second.
        for x in 0..area.width {
            let cell = &buf_scrolled[(x, 1)];
            assert!(cell.style().add_modifier.is_empty());
        }
    }

    #[tokio::test]
    async fn wrapped_line_meta_tracks_visual_lines() {
        let lines = vec![Line::from("abcd efgh")];
        let line_meta = vec![TranscriptLineMeta::CellLine {
            cell_index: 0,
            line_in_cell: 0,
        }];

        let wrapped_meta = App::build_wrapped_line_meta(&lines, &line_meta, 4);

        assert_eq!(
            wrapped_meta,
            vec![
                TranscriptLineMeta::CellLine {
                    cell_index: 0,
                    line_in_cell: 0,
                },
                TranscriptLineMeta::CellLine {
                    cell_index: 0,
                    line_in_cell: 1,
                },
            ],
        );
    }

    #[tokio::test]
    async fn new_session_requests_shutdown_for_previous_conversation() {
        let (mut app, mut app_event_rx, mut op_rx) = make_test_app_with_channels().await;

        let conversation_id = ConversationId::new();
        let event = SessionConfiguredEvent {
            session_id: conversation_id,
            model: "gpt-test".to_string(),
            model_provider_id: "test-provider".to_string(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::ReadOnly,
            cwd: PathBuf::from("/home/user/project"),
            reasoning_effort: None,
            history_log_id: 0,
            history_entry_count: 0,
            initial_messages: None,
            skill_load_outcome: None,
            rollout_path: PathBuf::new(),
        };

        app.chat_widget.handle_codex_event(Event {
            id: String::new(),
            msg: EventMsg::SessionConfigured(event),
        });

        while app_event_rx.try_recv().is_ok() {}
        while op_rx.try_recv().is_ok() {}

        app.shutdown_current_conversation().await;

        match op_rx.try_recv() {
            Ok(Op::Shutdown) => {}
            Ok(other) => panic!("expected Op::Shutdown, got {other:?}"),
            Err(_) => panic!("expected shutdown op to be sent"),
        }
    }

    #[tokio::test]
    async fn session_summary_skip_zero_usage() {
        assert!(session_summary(TokenUsage::default(), None).is_none());
    }

    #[tokio::test]
    async fn render_lines_to_ansi_pads_user_rows_to_full_width() {
        let line: Line<'static> = Line::from("hi");
        let lines = vec![line];
        let line_meta = vec![TranscriptLineMeta::CellLine {
            cell_index: 0,
            line_in_cell: 0,
        }];
        let is_user_cell = vec![true];
        let width: u16 = 10;

        let rendered = App::render_lines_to_ansi(&lines, &line_meta, &is_user_cell, width);
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].contains("hi"));
    }

    #[tokio::test]
    async fn session_summary_includes_resume_hint() {
        let usage = TokenUsage {
            input_tokens: 10,
            output_tokens: 2,
            total_tokens: 12,
            ..Default::default()
        };
        let conversation =
            ConversationId::from_string("123e4567-e89b-12d3-a456-426614174000").unwrap();

        let summary = session_summary(usage, Some(conversation)).expect("summary");
        assert_eq!(
            summary.usage_line,
            "Token usage: total=12 input=10 output=2"
        );
        assert_eq!(
            summary.resume_command,
            Some("codex resume 123e4567-e89b-12d3-a456-426614174000".to_string())
        );
    }
}
