use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_app_server_protocol::AuthMode;
use codex_backend_client::Client as BackendClient;
use codex_core::config::Config;
use codex_core::config::types::Notifications;
use codex_core::git_info::current_branch_name;
use codex_core::git_info::local_git_branches;
use codex_core::openai_models::model_family::ModelFamily;
use codex_core::openai_models::models_manager::ModelsManager;
use codex_core::project_doc::DEFAULT_PROJECT_DOC_FILENAME;
use codex_core::protocol::AgentMessageDeltaEvent;
use codex_core::protocol::AgentMessageEvent;
use codex_core::protocol::AgentReasoningDeltaEvent;
use codex_core::protocol::AgentReasoningEvent;
use codex_core::protocol::AgentReasoningRawContentDeltaEvent;
use codex_core::protocol::AgentReasoningRawContentEvent;
use codex_core::protocol::ApplyPatchApprovalRequestEvent;
use codex_core::protocol::BackgroundEventEvent;
use codex_core::protocol::CreditsSnapshot;
use codex_core::protocol::DeprecationNoticeEvent;
use codex_core::protocol::ErrorEvent;
use codex_core::protocol::Event;
use codex_core::protocol::EventMsg;
use codex_core::protocol::ExecApprovalRequestEvent;
use codex_core::protocol::ExecCommandBeginEvent;
use codex_core::protocol::ExecCommandEndEvent;
use codex_core::protocol::ExecCommandSource;
use codex_core::protocol::ExitedReviewModeEvent;
use codex_core::protocol::ListCustomPromptsResponseEvent;
use codex_core::protocol::ListSkillsResponseEvent;
use codex_core::protocol::McpListToolsResponseEvent;
use codex_core::protocol::McpStartupCompleteEvent;
use codex_core::protocol::McpStartupStatus;
use codex_core::protocol::McpStartupUpdateEvent;
use codex_core::protocol::McpToolCallBeginEvent;
use codex_core::protocol::McpToolCallEndEvent;
use codex_core::protocol::Op;
use codex_core::protocol::PatchApplyBeginEvent;
use codex_core::protocol::RateLimitSnapshot;
use codex_core::protocol::RawResponseItemEvent;
use codex_core::protocol::ReviewRequest;
use codex_core::protocol::ReviewTarget;
use codex_core::protocol::StreamErrorEvent;
use codex_core::protocol::TaskCompleteEvent;
use codex_core::protocol::TerminalInteractionEvent;
use codex_core::protocol::TokenUsage;
use codex_core::protocol::TokenUsageInfo;
use codex_core::protocol::TurnAbortReason;
use codex_core::protocol::TurnDiffEvent;
use codex_core::protocol::UndoCompletedEvent;
use codex_core::protocol::UndoStartedEvent;
use codex_core::protocol::UserMessageEvent;
use codex_core::protocol::ViewImageToolCallEvent;
use codex_core::protocol::WarningEvent;
use codex_core::protocol::WebSearchBeginEvent;
use codex_core::protocol::WebSearchEndEvent;
use codex_protocol::ConversationId;
use codex_protocol::account::PlanType;
use codex_protocol::approvals::ElicitationRequestEvent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::RalphCompletionReason;
use codex_protocol::protocol::RalphLoopCompleteEvent;
use codex_protocol::protocol::RalphLoopContinueEvent;
use codex_protocol::protocol::RalphLoopState;
use codex_protocol::protocol::RalphLoopStatusEvent;
use codex_protocol::user_input::UserInput;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use rand::Rng;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tracing::debug;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::ApprovalRequest;
use crate::bottom_pane::BottomPane;
use crate::bottom_pane::BottomPaneParams;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::InputResult;
use crate::bottom_pane::SelectionAction;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::custom_prompt_view::CustomPromptView;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::clipboard_paste::paste_image_to_temp_png;
use crate::diff_render::display_path_for;
use crate::exec_cell::CommandOutput;
use crate::exec_cell::ExecCell;
use crate::exec_cell::new_active_exec_command;
use crate::get_git_diff::get_git_diff;
use crate::history_cell;
use crate::history_cell::AgentMessageCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::McpToolCallCell;
use crate::history_cell::PlainHistoryCell;
use crate::markdown::append_markdown;
use crate::render::Insets;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::FlexRenderable;
use crate::render::renderable::Renderable;
use crate::render::renderable::RenderableExt;
use crate::render::renderable::RenderableItem;
use crate::slash_command::SlashCommand;
use crate::status::RateLimitSnapshotDisplay;
use crate::text_formatting::truncate_text;
use crate::tui::FrameRequester;
mod interrupts;
use self::interrupts::InterruptManager;
mod agent;
use self::agent::spawn_agent;
use self::agent::spawn_agent_from_existing;
mod session_header;
use self::session_header::SessionHeader;
use crate::streaming::controller::StreamController;
use std::fmt::Write;
use std::path::Path;
use std::time::SystemTime;
use uuid::Uuid;

use chrono::DateTime;
use chrono::Local;
use chrono::Utc;
use codex_common::approval_presets::ApprovalPreset;
use codex_common::approval_presets::builtin_approval_presets;
use codex_core::AuthManager;
use codex_core::CodexAuth;
use codex_core::ConversationManager;
use codex_core::protocol::AskForApproval;
use codex_core::protocol::SandboxPolicy;
use codex_file_search::FileMatch;
use codex_multi_agent::AgentId;
use codex_multi_agent::DelegateSessionMode;
use codex_multi_agent::DelegateSessionSummary;
use codex_multi_agent::DetachedRunStatusSummary;
use codex_multi_agent::DetachedRunSummary;
use codex_protocol::plan_tool::UpdatePlanArgs;
use strum::IntoEnumIterator;

use shlex::Shlex;

const USER_SHELL_COMMAND_HELP_TITLE: &str = "Prefix a command with ! to run it locally";
const USER_SHELL_COMMAND_HELP_HINT: &str = "Example: !ls";
// Track information about an in-flight exec command.
struct RunningCommand {
    command: Vec<String>,
    parsed_cmd: Vec<ParsedCommand>,
    source: ExecCommandSource,
}

struct UnifiedExecWaitState {
    command_display: String,
}

impl UnifiedExecWaitState {
    fn new(command_display: String) -> Self {
        Self { command_display }
    }

    fn is_duplicate(&self, command_display: &str) -> bool {
        self.command_display == command_display
    }
}

const RATE_LIMIT_WARNING_THRESHOLDS: [f64; 3] = [75.0, 90.0, 95.0];
const NUDGE_MODEL_SLUG: &str = "gpt-5.1-codex-mini";
const RATE_LIMIT_SWITCH_PROMPT_THRESHOLD: f64 = 90.0;

#[derive(Default)]
struct RateLimitWarningState {
    secondary_index: usize,
    primary_index: usize,
}

impl RateLimitWarningState {
    fn take_warnings(
        &mut self,
        secondary_used_percent: Option<f64>,
        secondary_window_minutes: Option<i64>,
        primary_used_percent: Option<f64>,
        primary_window_minutes: Option<i64>,
    ) -> Vec<String> {
        let reached_secondary_cap =
            matches!(secondary_used_percent, Some(percent) if percent == 100.0);
        let reached_primary_cap = matches!(primary_used_percent, Some(percent) if percent == 100.0);
        if reached_secondary_cap || reached_primary_cap {
            return Vec::new();
        }

        let mut warnings = Vec::new();

        if let Some(secondary_used_percent) = secondary_used_percent {
            let mut highest_secondary: Option<f64> = None;
            while self.secondary_index < RATE_LIMIT_WARNING_THRESHOLDS.len()
                && secondary_used_percent >= RATE_LIMIT_WARNING_THRESHOLDS[self.secondary_index]
            {
                highest_secondary = Some(RATE_LIMIT_WARNING_THRESHOLDS[self.secondary_index]);
                self.secondary_index += 1;
            }
            if let Some(threshold) = highest_secondary {
                let remaining = 100.0 - threshold;
                let limit_label = secondary_window_minutes
                    .map(get_limits_duration)
                    .unwrap_or_else(|| "weekly".to_string());
                warnings.push(format!(
                    "Heads up, you have less than {remaining:.0}% of your {limit_label} limit left. Run /status for a breakdown."
                ));
            }
        }

        if let Some(primary_used_percent) = primary_used_percent {
            let mut highest_primary: Option<f64> = None;
            while self.primary_index < RATE_LIMIT_WARNING_THRESHOLDS.len()
                && primary_used_percent >= RATE_LIMIT_WARNING_THRESHOLDS[self.primary_index]
            {
                highest_primary = Some(RATE_LIMIT_WARNING_THRESHOLDS[self.primary_index]);
                self.primary_index += 1;
            }
            if let Some(threshold) = highest_primary {
                let remaining = 100.0 - threshold;
                let limit_label = primary_window_minutes
                    .map(get_limits_duration)
                    .unwrap_or_else(|| "5h".to_string());
                warnings.push(format!(
                    "Heads up, you have less than {remaining:.0}% of your {limit_label} limit left. Run /status for a breakdown."
                ));
            }
        }

        warnings
    }
}

pub(crate) fn get_limits_duration(windows_minutes: i64) -> String {
    const MINUTES_PER_HOUR: i64 = 60;
    const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
    const MINUTES_PER_WEEK: i64 = 7 * MINUTES_PER_DAY;
    const MINUTES_PER_MONTH: i64 = 30 * MINUTES_PER_DAY;
    const ROUNDING_BIAS_MINUTES: i64 = 3;

    let windows_minutes = windows_minutes.max(0);

    if windows_minutes <= MINUTES_PER_DAY.saturating_add(ROUNDING_BIAS_MINUTES) {
        let adjusted = windows_minutes.saturating_add(ROUNDING_BIAS_MINUTES);
        let hours = std::cmp::max(1, adjusted / MINUTES_PER_HOUR);
        format!("{hours}h")
    } else if windows_minutes <= MINUTES_PER_WEEK.saturating_add(ROUNDING_BIAS_MINUTES) {
        "weekly".to_string()
    } else if windows_minutes <= MINUTES_PER_MONTH.saturating_add(ROUNDING_BIAS_MINUTES) {
        "monthly".to_string()
    } else {
        "annual".to_string()
    }
}

/// Common initialization parameters shared by all `ChatWidget` constructors.
pub(crate) struct ChatWidgetInit {
    pub(crate) config: Config,
    pub(crate) frame_requester: FrameRequester,
    pub(crate) app_event_tx: AppEventSender,
    pub(crate) initial_prompt: Option<String>,
    pub(crate) initial_images: Vec<PathBuf>,
    pub(crate) enhanced_keys_supported: bool,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) models_manager: Arc<ModelsManager>,
    pub(crate) feedback: codex_feedback::CodexFeedback,
    pub(crate) is_first_run: bool,
    pub(crate) model_family: ModelFamily,
}

#[derive(Clone, Debug)]
pub struct DelegateDisplayLabel {
    pub depth: usize,
    pub base_label: String,
}

#[derive(Clone)]
pub struct DelegatePickerSession {
    pub summary: DelegateSessionSummary,
    pub run_id: Option<String>,
}
#[derive(Default)]
enum RateLimitSwitchPromptState {
    #[default]
    Idle,
    Pending,
    Shown,
}

pub(crate) struct ChatWidget {
    app_event_tx: AppEventSender,
    codex_op_tx: UnboundedSender<Op>,
    bottom_pane: BottomPane,
    active_cell: Option<Box<dyn HistoryCell>>,
    config: Config,
    model_family: ModelFamily,
    auth_manager: Arc<AuthManager>,
    models_manager: Arc<ModelsManager>,
    session_header: SessionHeader,
    initial_user_message: Option<UserMessage>,
    defer_initial_message_for_alias_input: bool,
    token_info: Option<TokenUsageInfo>,
    rate_limit_snapshot: Option<RateLimitSnapshotDisplay>,
    plan_type: Option<PlanType>,
    rate_limit_warnings: RateLimitWarningState,
    rate_limit_switch_prompt: RateLimitSwitchPromptState,
    rate_limit_poller: Option<JoinHandle<()>>,
    // Stream lifecycle controller
    stream_controller: Option<StreamController>,
    running_commands: HashMap<String, RunningCommand>,
    suppressed_exec_calls: HashSet<String>,
    last_unified_wait: Option<UnifiedExecWaitState>,
    task_complete_pending: bool,
    mcp_startup_status: Option<HashMap<String, McpStartupStatus>>,
    // Queue of interruptive UI events deferred during an active write cycle
    interrupts: InterruptManager,
    // Accumulates the current reasoning block text to extract a header
    reasoning_buffer: String,
    // Accumulates full reasoning content for transcript-only recording
    full_reasoning_buffer: String,
    // Current status header shown in the status indicator.
    current_status_header: String,
    // Previous status header to restore after a transient stream retry.
    retry_status_header: Option<String>,
    conversation_id: Option<ConversationId>,
    frame_requester: FrameRequester,
    // Whether to include the initial welcome banner on session configured
    show_welcome_banner: bool,
    // When resuming an existing session (selected via resume picker), avoid an
    // immediate redraw on SessionConfigured to prevent a gratuitous UI flicker.
    suppress_session_configured_redraw: bool,
    // User messages queued while a turn is in progress
    queued_user_messages: VecDeque<UserMessage>,
    queued_turn_pending_start: bool,
    // Last non-empty user message text (used by commands that default to "repeat last prompt").
    last_user_message: Option<String>,
    // Active Ralph loop state (if enabled via `/ralph-loop`).
    ralph_loop_state: Option<RalphLoopState>,
    // Pending notification to show when unfocused on next Draw
    pending_notification: Option<Notification>,
    // Simple review mode flag; used to adjust layout and banners.
    is_review_mode: bool,
    // Snapshot of token usage to restore after review mode exits.
    pre_review_token_info: Option<Option<TokenUsageInfo>>,
    // Whether to add a final message separator after the last message
    needs_final_message_separator: bool,

    delegate_run: Option<String>,
    delegate_runs_with_stream: HashSet<String>,
    delegate_status_owner: Option<String>,
    delegate_previous_status_header: Option<String>,
    delegate_context: Option<DelegateSessionSummary>,
    delegate_user_frames: Vec<codex_protocol::user_input::UserInput>,
    delegate_agent_frames: Vec<String>,
    pending_delegate_context: Vec<String>,

    last_rendered_width: std::cell::Cell<Option<usize>>,
    // Feedback sink for /feedback
    feedback: codex_feedback::CodexFeedback,
    // Current session rollout path (if known)
    current_rollout_path: Option<PathBuf>,
    // Monotonic counter for generated images in this session.
    next_generated_image_index: u64,
    // Last generated image path for quick reopening via slash command.
    last_generated_image_path: Option<PathBuf>,
    // UI-level view of the active reference image set for this session.
    ref_images: RefImageManager,
    // Batch image processing state
    batch_image_state: Option<BatchImageState>,
    // Pending PDF update state for async processing
    pending_pdf_update: Option<PendingPdfUpdate>,
}

struct UserMessage {
    text: String,
    image_paths: Vec<PathBuf>,
}

/// Manager for the reference image set used by `/ref-image`.
///
/// This tracks the UI's view of the active reference images as local paths.
/// Core maintains its own data URL representation; the two are kept in sync
/// via `Op::SetReferenceImages` / `Op::ClearReferenceImages`.
struct RefImageManager {
    active_paths: Vec<PathBuf>,
}

/// State for batch image processing via `/ref-image-batch`.
struct BatchImageState {
    /// Directory containing images to process.
    source_dir: PathBuf,
    /// Queue of remaining image paths to process.
    pending_images: VecDeque<PathBuf>,
    /// The prompt to use for each image.
    prompt: String,
    /// Total count for progress display.
    total_count: usize,
    /// Number of images successfully processed.
    processed_count: usize,
    /// Currently processing image path.
    current_image: Option<PathBuf>,
    /// If this is a PDF update operation, the original PDF path for output.
    original_pdf_path: Option<PathBuf>,
}

impl BatchImageState {
    fn new(source_dir: PathBuf, images: Vec<PathBuf>, prompt: String) -> Self {
        let total_count = images.len();
        Self {
            source_dir,
            pending_images: images.into(),
            prompt,
            total_count,
            processed_count: 0,
            current_image: None,
            original_pdf_path: None,
        }
    }

    fn new_for_pdf(
        source_dir: PathBuf,
        images: Vec<PathBuf>,
        prompt: String,
        pdf_path: PathBuf,
    ) -> Self {
        let total_count = images.len();
        Self {
            source_dir,
            pending_images: images.into(),
            prompt,
            total_count,
            processed_count: 0,
            current_image: None,
            original_pdf_path: Some(pdf_path),
        }
    }

    fn next_image(&mut self) -> Option<PathBuf> {
        self.current_image = self.pending_images.pop_front();
        self.current_image.clone()
    }

    fn mark_current_processed(&mut self) {
        if self.current_image.is_some() {
            self.processed_count += 1;
            self.current_image = None;
        }
    }

    fn progress_message(&self) -> String {
        format!(
            "[Batch] Processing {}/{}: {:?}",
            self.processed_count + 1,
            self.total_count,
            self.current_image
                .as_ref()
                .map(|p| p.file_name().unwrap_or_default())
                .unwrap_or_default()
        )
    }

    fn completion_message(&self) -> String {
        format!(
            "[Batch] Complete! Processed {} images in {:?}",
            self.processed_count,
            self.source_dir.file_name().unwrap_or_default()
        )
    }
}

/// State for pending PDF update operation via `/pdf-update`.
struct PendingPdfUpdate {
    /// Path to the PDF file.
    pdf_path: PathBuf,
    /// Directory where processed images will be stored.
    images_output_dir: PathBuf,
    /// The prompt to use for batch image processing.
    prompt: String,
}

impl RefImageManager {
    fn new() -> Self {
        Self {
            active_paths: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.active_paths.clear();
    }

    fn set_paths(&mut self, paths: Vec<PathBuf>) {
        self.active_paths = paths;
    }

    fn active_paths(&self) -> &[PathBuf] {
        &self.active_paths
    }
}

struct RefImageContext<'a> {
    cwd: &'a Path,
    codex_home: &'a Path,
    conversation_id: Option<&'a ConversationId>,
}

enum RefImageCommand {
    ShowHelpAndMaybeStatus,
    ShowStatusOnly,
    Clear,
    Set {
        paths: Vec<PathBuf>,
        prompt: Option<String>,
    },
}

impl RefImageManager {
    fn parse_command(&self, args: Option<String>, ctx: &RefImageContext<'_>) -> RefImageCommand {
        let raw = args.unwrap_or_default();
        let trimmed = raw.trim();

        if trimmed.is_empty() {
            return RefImageCommand::ShowHelpAndMaybeStatus;
        }

        if trimmed.eq_ignore_ascii_case("ls") {
            return RefImageCommand::ShowStatusOnly;
        }

        if trimmed.eq_ignore_ascii_case("clear") {
            return RefImageCommand::Clear;
        }

        let (paths_raw, prompt_raw) = if let Some((left, right)) = trimmed.split_once("--") {
            (left.trim_end(), Some(right.trim_start().to_string()))
        } else {
            (trimmed, None)
        };

        let path_tokens: Vec<String> = Shlex::new(paths_raw).filter(|s| !s.is_empty()).collect();
        if path_tokens.is_empty() {
            // If the user supplied only a prompt (for example `/ref-image -- tweak the style`)
            // defer image selection to the caller so it can integrate any attached images.
            if let Some(prompt_raw) = prompt_raw {
                let prompt = {
                    let trimmed = prompt_raw.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                };
                return RefImageCommand::Set {
                    paths: Vec::new(),
                    prompt,
                };
            }
            return RefImageCommand::ShowHelpAndMaybeStatus;
        }

        let mut resolved: Vec<PathBuf> = Vec::new();
        for token in path_tokens {
            resolved.push(Self::resolve_path(&token, ctx));
        }

        let prompt = prompt_raw.and_then(|p| {
            let trimmed = p.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        RefImageCommand::Set {
            paths: resolved,
            prompt,
        }
    }

    fn resolve_path(raw: &str, ctx: &RefImageContext<'_>) -> PathBuf {
        // Expand ~/ prefix against the user's home directory when possible.
        let expanded = if let Some(stripped) = raw.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(stripped)
            } else {
                PathBuf::from(raw)
            }
        } else {
            PathBuf::from(raw)
        };

        if expanded.is_absolute() {
            return expanded;
        }

        // If the path has multiple components, treat it as relative to the current
        // working directory (e.g. subdir/image.png).
        let mut components = expanded.components();
        if components.next().is_some() && components.next().is_some() {
            return ctx.cwd.join(expanded);
        }

        // Single-segment relative path (e.g. "000000.png"): prefer the session's
        // images directory when available so users can omit the full ~/.codex path.
        if let Some(conversation_id) = ctx.conversation_id {
            let candidate = ctx
                .codex_home
                .join("images")
                .join(conversation_id.to_string())
                .join(&expanded);
            if candidate.exists() {
                return candidate;
            }
        }

        ctx.cwd.join(expanded)
    }
}

#[derive(Default)]
pub(crate) struct DelegateCapture {
    pub user_inputs: Vec<codex_protocol::user_input::UserInput>,
    pub agent_outputs: Vec<String>,
}

impl DelegateCapture {
    fn is_empty(&self) -> bool {
        self.user_inputs.is_empty() && self.agent_outputs.is_empty()
    }
}

impl From<String> for UserMessage {
    fn from(text: String) -> Self {
        Self {
            text,
            image_paths: Vec::new(),
        }
    }
}

impl From<&str> for UserMessage {
    fn from(text: &str) -> Self {
        Self {
            text: text.to_string(),
            image_paths: Vec::new(),
        }
    }
}

fn create_initial_user_message(text: String, image_paths: Vec<PathBuf>) -> Option<UserMessage> {
    if text.is_empty() && image_paths.is_empty() {
        None
    } else {
        Some(UserMessage { text, image_paths })
    }
}

impl ChatWidget {
    pub(crate) fn handle_ref_image_command(&mut self, args: Option<String>) {
        let ctx = RefImageContext {
            cwd: &self.config.cwd,
            codex_home: &self.config.codex_home,
            conversation_id: self.conversation_id.as_ref(),
        };

        match self.ref_images.parse_command(args, &ctx) {
            RefImageCommand::ShowHelpAndMaybeStatus => {
                let message = "Usage: /ref-image <path1> <path2> ... [-- <prompt>]\n\
                               • `/ref-image ls` — show current reference images\n\
                               • `/ref-image clear` — clear the active reference images";
                self.add_info_message(message.to_string(), None);

                // If there is an active set, show it after the help text.
                if !self.ref_images.active_paths().is_empty() {
                    let display_paths: Vec<String> = self
                        .ref_images
                        .active_paths()
                        .iter()
                        .map(|p| display_path_for(p, &self.config.cwd))
                        .collect();
                    self.add_info_message(
                        format!("Active reference images: {}", display_paths.join(", ")),
                        None,
                    );
                }
            }
            RefImageCommand::ShowStatusOnly => {
                if self.ref_images.active_paths().is_empty() {
                    self.add_info_message(
                        "No active reference images. The model will infer references from recent images."
                            .to_string(),
                        None,
                    );
                } else {
                    let display_paths: Vec<String> = self
                        .ref_images
                        .active_paths()
                        .iter()
                        .map(|p| display_path_for(p, &self.config.cwd))
                        .collect();
                    self.add_info_message(
                        format!("Active reference images: {}", display_paths.join(", ")),
                        None,
                    );
                }
            }
            RefImageCommand::Clear => {
                self.ref_images.clear();
                // Forward ClearReferenceImages directly so it is ordered
                // consistently with any subsequent user input.
                self.submit_op(Op::ClearReferenceImages);
                self.add_info_message("Reference images cleared.".to_string(), None);
            }
            RefImageCommand::Set { paths, prompt } => {
                // If the user pasted or attached images in the composer and then
                // invoked `/ref-image`, prefer those attached image paths over
                // any literal placeholders that may appear in the command text.
                // This keeps `/ref-image` aligned with the image attachment
                // pipeline used elsewhere in the TUI.
                let attached_paths = self.bottom_pane.take_recent_submission_images();
                let final_paths = if attached_paths.is_empty() {
                    paths
                } else {
                    attached_paths
                };

                self.ref_images.set_paths(final_paths.clone());
                // Ensure the reference images are updated in core before
                // sending any prompt that might rely on them. Using
                // `submit_op` keeps the ordering consistent with the
                // subsequent `Op::UserInput`.
                self.submit_op(Op::SetReferenceImages { paths: final_paths });

                let display_paths: Vec<String> = self
                    .ref_images
                    .active_paths()
                    .iter()
                    .map(|p| display_path_for(p, &self.config.cwd))
                    .collect();
                self.add_info_message(
                    format!("Reference images set: {}", display_paths.join(", ")),
                    None,
                );

                if let Some(prompt_text) = prompt {
                    let user_message = UserMessage {
                        text: prompt_text,
                        image_paths: Vec::new(),
                    };
                    self.queue_user_message(user_message);
                }
            }
        }
    }

    pub(crate) fn handle_image_quality_command(&mut self, args: Option<String>) {
        let valid_options = "1K, 2K, 4K";

        let size_arg = args.as_deref().map(|s| s.trim().to_uppercase());
        match size_arg {
            None => {
                let message = format!(
                    "Usage: /image-quality <size>\n\
                     • `1K` — 1024x1024 (default, fastest)\n\
                     • `2K` — 2048x2048 (balanced)\n\
                     • `4K` — 4096x4096 (highest quality, slower)\n\n\
                     Valid options: {valid_options}"
                );
                self.add_info_message(message, None);
            }
            Some(ref s) if s.is_empty() => {
                let message = format!(
                    "Usage: /image-quality <size>\n\
                     • `1K` — 1024x1024 (default, fastest)\n\
                     • `2K` — 2048x2048 (balanced)\n\
                     • `4K` — 4096x4096 (highest quality, slower)\n\n\
                     Valid options: {valid_options}"
                );
                self.add_info_message(message, None);
            }
            Some(size) => {
                if matches!(size.as_str(), "1K" | "2K" | "4K") {
                    self.submit_op(Op::SetImageQuality { size: size.clone() });
                    self.add_info_message(format!("Image quality set to {size}"), None);
                } else {
                    self.add_info_message(
                        format!("Invalid image quality '{size}'. Valid options: {valid_options}"),
                        None,
                    );
                }
            }
        }
    }

    /// Handle `/ref-image-batch <path> -- <prompt>` command for batch image processing.
    pub(crate) fn handle_ref_image_batch_command(&mut self, args: Option<String>) {
        let raw = args.unwrap_or_default();
        let trimmed = raw.trim();

        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("help") {
            let help = "Usage: /ref-image-batch <folder_path> -- <prompt>\n\n\
                        Batch process all images in a folder with the same prompt.\n\
                        Images are processed one by one to avoid rate limits.\n\
                        Output images are saved as `<original_name>_processed.<ext>`.\n\n\
                        Supported formats: .png, .jpg, .jpeg, .webp, .gif\n\n\
                        Example:\n\
                        /ref-image-batch ./photos -- Convert to oil painting style";
            self.add_info_message(help.to_string(), None);
            return;
        }

        // Parse: <path> -- <prompt>
        let Some((path_raw, prompt_raw)) = trimmed.split_once("--") else {
            self.add_info_message(
                "Error: Missing prompt. Use: /ref-image-batch <folder> -- <prompt>".to_string(),
                None,
            );
            return;
        };

        let path_str = path_raw.trim();
        let prompt = prompt_raw.trim().to_string();

        if prompt.is_empty() {
            self.add_info_message("Error: Prompt cannot be empty.".to_string(), None);
            return;
        }

        // Resolve the path
        let source_dir = if path_str.starts_with('/') {
            PathBuf::from(path_str)
        } else if let Some(stripped) = path_str.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(stripped)
            } else {
                self.config.cwd.join(path_str)
            }
        } else {
            self.config.cwd.join(path_str)
        };

        if !source_dir.exists() {
            self.add_info_message(
                format!("Error: Path does not exist: {}", source_dir.display()),
                None,
            );
            return;
        }

        if !source_dir.is_dir() {
            self.add_info_message(
                format!("Error: Path is not a directory: {}", source_dir.display()),
                None,
            );
            return;
        }

        // Scan for image files
        let image_extensions = ["png", "jpg", "jpeg", "webp", "gif"];
        let mut images: Vec<PathBuf> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&source_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && let Some(ext) = path.extension().and_then(|e| e.to_str())
                {
                    let ext_lower = ext.to_lowercase();
                    // Skip already processed files
                    if !path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.ends_with("_processed"))
                        .unwrap_or(false)
                        && image_extensions.contains(&ext_lower.as_str())
                    {
                        images.push(path);
                    }
                }
            }
        }

        if images.is_empty() {
            self.add_info_message(
                format!(
                    "No images found in: {}\nSupported formats: {:?}",
                    source_dir.display(),
                    image_extensions
                ),
                None,
            );
            return;
        }

        // Sort by filename for consistent ordering
        images.sort();

        let total = images.len();
        self.add_info_message(
            format!(
                "[Batch] Starting batch processing of {} images in {}\nPrompt: {}",
                total,
                source_dir.display(),
                prompt
            ),
            None,
        );

        // Initialize batch state
        self.batch_image_state = Some(BatchImageState::new(source_dir, images, prompt));

        // Start processing the first image
        self.process_next_batch_image();
    }

    /// Handle the /pdf-update command for PDF watermark removal and batch processing.
    ///
    /// Usage: /pdf-update <pdf_path> -- <prompt>
    ///
    /// This command:
    /// 1. Converts PDF to images and removes watermarks (via watermark-remover MCP server)
    /// 2. Processes each image with the given prompt (via Gemini)
    /// 3. Merges processed images back into a PDF
    pub(crate) fn handle_pdf_update_command(&mut self, args: Option<String>) {
        let raw = args.unwrap_or_default();
        let trimmed = raw.trim();

        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("help") {
            let help = "Usage: /pdf-update <pdf_path> -- <prompt>\n\n\
                        Process PDF: remove watermarks and apply image transformations.\n\n\
                        ═══════════════════════════════════════════════════════════════\n\
                        SETUP REQUIRED:\n\
                        ═══════════════════════════════════════════════════════════════\n\n\
                        1. Install Python dependencies:\n\
                           pip install pdf2image img2pdf opencv-python-headless numpy Pillow\n\n\
                        2. Install poppler (for PDF processing):\n\
                           macOS:  brew install poppler\n\
                           Ubuntu: sudo apt install poppler-utils\n\n\
                        3. Configure MCP server in ~/.codex/config.toml:\n\
                           [mcp_servers.watermark-remover]\n\
                           command = \"/path/to/watermark-remover-mcp-server\"\n\
                           env = { WATERMARK_SCRIPTS_DIR = \"/path/to/scripts\" }\n\n\
                        ═══════════════════════════════════════════════════════════════\n\n\
                        Example:\n\
                        /pdf-update ~/Documents/report.pdf -- Convert to oil painting style";
            self.add_info_message(help.to_string(), None);
            return;
        }

        // Parse: <path> -- <prompt>
        let Some((path_raw, prompt_raw)) = trimmed.split_once("--") else {
            self.add_info_message(
                "Error: Missing prompt. Use: /pdf-update <pdf_path> -- <prompt>".to_string(),
                None,
            );
            return;
        };

        let path_str = path_raw.trim();
        let prompt = prompt_raw.trim().to_string();

        if prompt.is_empty() {
            self.add_info_message("Error: Prompt cannot be empty.".to_string(), None);
            return;
        }

        // Resolve the PDF path
        let pdf_path = if path_str.starts_with('/') {
            PathBuf::from(path_str)
        } else if let Some(stripped) = path_str.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(stripped)
            } else {
                self.config.cwd.join(path_str)
            }
        } else {
            self.config.cwd.join(path_str)
        };

        if !pdf_path.exists() {
            self.add_info_message(
                format!("Error: PDF file does not exist: {}", pdf_path.display()),
                None,
            );
            return;
        }

        if !pdf_path.is_file() {
            self.add_info_message(
                format!("Error: Path is not a file: {}", pdf_path.display()),
                None,
            );
            return;
        }

        // Check file extension
        let extension = pdf_path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);

        if extension.as_deref() != Some("pdf") {
            self.add_info_message(
                format!(
                    "Error: File does not appear to be a PDF: {}",
                    pdf_path.display()
                ),
                None,
            );
            return;
        }

        // Build the images output directory path: ~/.codex/images/{session_id}/{pdf_stem}_images/
        let pdf_stem = pdf_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pdf");
        let session_id = self
            .conversation_id
            .as_ref()
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| "unknown".to_string());
        let images_output_dir = self
            .config
            .codex_home
            .join("images")
            .join(&session_id)
            .join(format!("{pdf_stem}_images"));

        self.add_info_message(
            format!(
                "[PDF Update] Starting PDF processing...\n\
                 Input: {}\n\
                 Output: {}\n\
                 Prompt: {}\n\n\
                 Step 1: Removing watermarks...",
                pdf_path.display(),
                images_output_dir.display(),
                prompt
            ),
            None,
        );

        // Store the pending PDF update state for async processing
        self.pending_pdf_update = Some(PendingPdfUpdate {
            pdf_path,
            images_output_dir,
            prompt,
        });

        // Trigger the async PDF processing
        self.start_pdf_watermark_removal();
    }

    /// Start the async PDF watermark removal process
    fn start_pdf_watermark_removal(&mut self) {
        let Some(pending) = self.pending_pdf_update.as_ref() else {
            return;
        };

        let pdf_path = pending.pdf_path.clone();
        let images_output_dir = pending.images_output_dir.clone();

        // Create output directory
        if let Err(e) = std::fs::create_dir_all(&images_output_dir) {
            self.add_info_message(format!("Error creating output directory: {e}"), None);
            self.pending_pdf_update = None;
            return;
        }

        // Embedded Python script for PDF processing and watermark removal
        const PDF_PROCESS_SCRIPT: &str = r#"
import sys
import os
from pathlib import Path

def main():
    if len(sys.argv) < 3:
        print("Usage: python script.py <input_pdf> <output_dir> [dpi]", file=sys.stderr)
        sys.exit(1)

    input_pdf = sys.argv[1]
    output_dir = sys.argv[2]
    dpi = int(sys.argv[3]) if len(sys.argv) > 3 else 200

    if not os.path.exists(input_pdf):
        print(f"Error: Input PDF not found: {input_pdf}", file=sys.stderr)
        sys.exit(1)

    try:
        from pdf2image import convert_from_path
        import cv2
        import numpy as np
    except ImportError as e:
        print(f"Error: Missing dependency: {e}", file=sys.stderr)
        print("Run: pip install pdf2image opencv-python-headless numpy", file=sys.stderr)
        sys.exit(1)

    Path(output_dir).mkdir(parents=True, exist_ok=True)

    print(f"Converting PDF to images (DPI={dpi})...")
    try:
        images = convert_from_path(input_pdf, dpi=dpi)
    except Exception as e:
        print(f"Error converting PDF: {e}", file=sys.stderr)
        print("Make sure poppler is installed (brew install poppler)", file=sys.stderr)
        sys.exit(1)

    print(f"Total pages: {len(images)}")
    print("Removing watermarks...")
    processed_count = 0

    for i, image in enumerate(images):
        temp_path = os.path.join(output_dir, f"_temp_{i}.png")
        output_path = os.path.join(output_dir, f"page_{i+1:03d}.png")
        image.save(temp_path, "PNG")

        img = cv2.imread(temp_path)
        height, width = img.shape[:2]
        roi_x, roi_y = int(width * 0.80), int(height * 0.92)
        roi = img[roi_y:height, roi_x:width]
        gray_roi = cv2.cvtColor(roi, cv2.COLOR_BGR2GRAY)
        mask_roi = cv2.inRange(gray_roi, 150, 240)
        kernel = cv2.getStructuringElement(cv2.MORPH_RECT, (5, 5))
        mask_roi = cv2.dilate(mask_roi, kernel, iterations=2)
        mask = np.zeros((height, width), dtype=np.uint8)
        mask[roi_y:height, roi_x:width] = mask_roi

        if np.sum(mask) > 100:
            kernel_expand = cv2.getStructuringElement(cv2.MORPH_RECT, (7, 7))
            mask = cv2.dilate(mask, kernel_expand, iterations=1)
            result = cv2.inpaint(img, mask, inpaintRadius=5, flags=cv2.INPAINT_TELEA)
            cv2.imwrite(output_path, result)
            processed_count += 1
            print(f"  page_{i+1:03d}.png: watermark removed")
        else:
            cv2.imwrite(output_path, img)
            print(f"  page_{i+1:03d}.png: no watermark")
        os.remove(temp_path)

    print(f"Done! {len(images)} pages, {processed_count} watermarks removed")

if __name__ == "__main__":
    main()
"#;

        // Write script to temp file and execute
        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("codex_pdf_process.py");

        if let Err(e) = std::fs::write(&script_path, PDF_PROCESS_SCRIPT) {
            self.add_info_message(format!("Error writing temp script: {e}"), None);
            self.pending_pdf_update = None;
            return;
        }

        // Run Python script
        let output = std::process::Command::new("python3")
            .arg(&script_path)
            .arg(pdf_path.to_string_lossy().to_string())
            .arg(images_output_dir.to_string_lossy().to_string())
            .arg("200") // DPI
            .output();

        match output {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    self.add_info_message(format!("[PDF Update] Step 1 Complete!\n{stdout}"), None);
                    // Now start batch processing
                    self.start_batch_after_pdf_processing();
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    self.add_info_message(format!("Error processing PDF: {stderr}"), None);
                    self.pending_pdf_update = None;
                }
            }
            Err(e) => {
                self.add_info_message(format!("Error running Python script: {e}"), None);
                self.pending_pdf_update = None;
            }
        }
    }

    /// Start batch image processing after PDF watermark removal
    fn start_batch_after_pdf_processing(&mut self) {
        let Some(pending) = self.pending_pdf_update.take() else {
            return;
        };

        let source_dir = pending.images_output_dir;
        let prompt = pending.prompt;
        let pdf_path = pending.pdf_path;

        // Scan for image files
        let image_extensions = ["png", "jpg", "jpeg", "webp", "gif"];
        let mut images: Vec<PathBuf> = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&source_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && let Some(ext) = path.extension().and_then(|e| e.to_str())
                {
                    let ext_lower = ext.to_lowercase();
                    // Skip already processed files
                    if !path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s.ends_with("_processed"))
                        .unwrap_or(false)
                        && image_extensions.contains(&ext_lower.as_str())
                    {
                        images.push(path);
                    }
                }
            }
        }

        if images.is_empty() {
            self.add_info_message(
                format!(
                    "No images found after PDF processing in: {}",
                    source_dir.display()
                ),
                None,
            );
            return;
        }

        // Sort by filename for consistent ordering
        images.sort();

        let total = images.len();
        self.add_info_message(
            format!(
                "[PDF Update] Step 2: Starting batch processing of {} images\n\
                 Directory: {}\n\
                 Prompt: {}",
                total,
                source_dir.display(),
                prompt
            ),
            None,
        );

        // Initialize batch state with PDF path for final merge
        self.batch_image_state = Some(BatchImageState::new_for_pdf(
            source_dir, images, prompt, pdf_path,
        ));

        // Start processing the first image
        self.process_next_batch_image();
    }

    /// Process the next image in the batch queue.
    fn process_next_batch_image(&mut self) {
        // Extract values to avoid borrow checker issues
        let (image_path, progress_msg, prompt, batch_complete_info) = {
            let Some(batch_state) = self.batch_image_state.as_mut() else {
                return;
            };

            if let Some(image_path) = batch_state.next_image() {
                let progress = batch_state.progress_message();
                let prompt = batch_state.prompt.clone();
                (Some(image_path), Some(progress), Some(prompt), None)
            } else {
                // Batch is complete, extract info for PDF merge if needed
                let completion_msg = batch_state.completion_message();
                let pdf_info = batch_state
                    .original_pdf_path
                    .as_ref()
                    .map(|pdf_path| (batch_state.source_dir.clone(), pdf_path.clone()));
                (None, None, None, Some((completion_msg, pdf_info)))
            }
        };

        if let Some((completion_msg, pdf_info)) = batch_complete_info {
            self.batch_image_state = None;
            self.add_info_message(completion_msg, None);

            // If this was a PDF update, merge processed images back to PDF
            if let Some((source_dir, original_pdf_path)) = pdf_info {
                self.merge_processed_images_to_pdf(source_dir, original_pdf_path);
            }
            return;
        }

        if let (Some(image_path), Some(progress_msg), Some(prompt)) =
            (image_path, progress_msg, prompt)
        {
            // Show progress
            self.add_info_message(progress_msg, None);

            // Set the reference image
            self.ref_images.set_paths(vec![image_path.clone()]);
            self.submit_op(Op::SetReferenceImages {
                paths: vec![image_path],
            });

            // Submit the prompt
            let user_message = UserMessage {
                text: prompt,
                image_paths: Vec::new(),
            };
            self.queue_user_message(user_message);
        }
    }

    /// Called when a turn completes to continue batch processing if active.
    pub(crate) fn on_turn_complete_for_batch(&mut self) {
        if self.batch_image_state.is_some() {
            // Mark current as processed
            if let Some(batch_state) = self.batch_image_state.as_mut() {
                batch_state.mark_current_processed();
            }
            // Process next
            self.process_next_batch_image();
        }
    }

    /// Merge processed images back into a PPTX file
    fn merge_processed_images_to_pdf(&mut self, source_dir: PathBuf, original_pdf_path: PathBuf) {
        self.add_info_message(
            "[PDF Update] Step 3: Creating PPTX from processed images...".to_string(),
            None,
        );

        // Embedded Python script for merging images to PPTX
        const MERGE_SCRIPT: &str = r#"
import sys
import os

def main():
    if len(sys.argv) < 3:
        print("Usage: python script.py <image_dir> <output_pptx>", file=sys.stderr)
        sys.exit(1)

    image_dir = sys.argv[1]
    output_pptx = sys.argv[2]

    try:
        from pptx import Presentation
        from pptx.util import Inches, Pt
        from PIL import Image
    except ImportError as e:
        print(f"Error: Missing dependency: {e}", file=sys.stderr)
        print("Run: pip install python-pptx Pillow", file=sys.stderr)
        sys.exit(1)

    # Find all processed images
    images = sorted([
        os.path.join(image_dir, f) for f in os.listdir(image_dir)
        if f.endswith('_processed.png')
    ])

    if not images:
        print("Error: No processed images found", file=sys.stderr)
        sys.exit(1)

    print(f"Found {len(images)} processed images")

    # Create presentation
    prs = Presentation()

    # Get image dimensions from first image to set slide size
    with Image.open(images[0]) as img:
        img_width, img_height = img.size

    # Set slide size based on image aspect ratio (in EMUs)
    # Standard width is 10 inches
    slide_width = Inches(10)
    slide_height = Inches(10 * img_height / img_width)
    prs.slide_width = slide_width
    prs.slide_height = slide_height

    # Blank slide layout
    blank_layout = prs.slide_layouts[6]  # Usually the blank layout

    for img_path in images:
        slide = prs.slides.add_slide(blank_layout)

        # Add image to fill the slide
        slide.shapes.add_picture(
            img_path,
            Inches(0),
            Inches(0),
            width=slide_width,
            height=slide_height
        )
        print(f"  Added: {os.path.basename(img_path)}")

    prs.save(output_pptx)
    print(f"PPTX created: {output_pptx}")

if __name__ == "__main__":
    main()
"#;

        // Determine output PPTX path (same directory as original, with _processed suffix)
        let pdf_stem = original_pdf_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let output_pptx = original_pdf_path
            .parent()
            .unwrap_or(&original_pdf_path)
            .join(format!("{pdf_stem}_processed.pptx"));

        // Write script to temp file
        let temp_dir = std::env::temp_dir();
        let script_path = temp_dir.join("codex_merge_pptx.py");

        if let Err(e) = std::fs::write(&script_path, MERGE_SCRIPT) {
            self.add_info_message(format!("Error writing merge script: {e}"), None);
            return;
        }

        // Run Python script
        let output = std::process::Command::new("python3")
            .arg(&script_path)
            .arg(source_dir.to_string_lossy().to_string())
            .arg(output_pptx.to_string_lossy().to_string())
            .output();

        match output {
            Ok(output) => {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    self.add_info_message(
                        format!(
                            "[PDF Update] Complete!\n\
                             Output PPTX: {}\n\
                             {}",
                            output_pptx.display(),
                            stdout
                        ),
                        None,
                    );
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    self.add_info_message(format!("Error creating PPTX: {stderr}"), None);
                }
            }
            Err(e) => {
                self.add_info_message(format!("Error running merge script: {e}"), None);
            }
        }
    }

    fn flush_answer_stream_with_separator(&mut self) {
        if let Some(mut controller) = self.stream_controller.take()
            && let Some(cell) = controller.finalize()
        {
            self.add_boxed_history(cell);
        }
    }

    fn set_status_header(&mut self, header: String) {
        self.current_status_header = header.clone();
        self.bottom_pane.update_status_header(header);
    }

    // --- Small event handlers ---
    fn on_session_configured(&mut self, event: codex_core::protocol::SessionConfiguredEvent) {
        self.bottom_pane
            .set_history_metadata(event.history_log_id, event.history_entry_count);
        let session_id = event.session_id;
        self.conversation_id = Some(session_id);
        self.current_rollout_path = Some(event.rollout_path.clone());
        let initial_messages = event.initial_messages.clone();
        let model_for_header = event.model.clone();
        let requested_model = self
            .config
            .model
            .clone()
            .unwrap_or_else(|| model_for_header.clone());
        self.session_header.set_model(&model_for_header);
        self.config.model = Some(model_for_header.clone());
        self.config.model_provider_id = event.model_provider_id.clone();
        if let Err(err) = self.config.approval_policy.set(event.approval_policy) {
            tracing::warn!(%err, "failed to set approval_policy on session configured");
        }
        self.config.sandbox_policy = event.sandbox_policy.clone();
        self.config.model_reasoning_effort = event.reasoning_effort;

        // Check if this is a new session (no history) and show alias input
        let is_new_session = event.history_entry_count == 0 && initial_messages.is_none();

        self.add_to_history(history_cell::new_session_info(
            &self.config,
            &requested_model,
            event,
            self.show_welcome_banner,
        ));
        if let Some(messages) = initial_messages {
            self.replay_initial_messages(messages);
        }

        if is_new_session {
            self.defer_initial_message_for_alias_input = true;
            let app_tx = self.app_event_tx.clone();
            let sid = session_id.to_string();
            self.bottom_pane.show_session_alias_input(
                sid,
                Box::new(move |session_id, alias| {
                    app_tx.send(AppEvent::SaveSessionAlias { session_id, alias });
                }),
            );
        } else {
            self.defer_initial_message_for_alias_input = false;
            if let Some(user_message) = self.initial_user_message.take() {
                self.submit_user_message(user_message);
            }
        }

        // Ask codex-core to enumerate custom prompts for this session.
        self.submit_op(Op::ListCustomPrompts);
        if !self.suppress_session_configured_redraw {
            self.request_redraw();
        }
    }

    /// Show alias input dialog for renaming an existing session
    pub(crate) fn show_session_alias_input_for_rename(
        &mut self,
        session_id: String,
        on_submit: Box<dyn Fn(String, String) + Send + Sync>,
    ) {
        self.bottom_pane
            .show_session_alias_input(session_id, on_submit);
    }

    pub(crate) fn set_delegate_context(&mut self, summary: Option<DelegateSessionSummary>) {
        let label = summary
            .as_ref()
            .map(|s| format!("#{}", s.agent_id.as_str()));
        self.bottom_pane.set_delegate_label(label);
        self.delegate_context = summary;
        self.delegate_user_frames.clear();
        self.delegate_agent_frames.clear();
    }

    pub(crate) fn take_delegate_capture(&mut self) -> Option<DelegateCapture> {
        if self.delegate_user_frames.is_empty() && self.delegate_agent_frames.is_empty() {
            return None;
        }
        Some(DelegateCapture {
            user_inputs: std::mem::take(&mut self.delegate_user_frames),
            agent_outputs: std::mem::take(&mut self.delegate_agent_frames),
        })
    }

    pub(crate) fn apply_delegate_summary(
        &mut self,
        summary: &DelegateSessionSummary,
        capture: DelegateCapture,
    ) {
        if capture.is_empty() {
            self.add_info_message(
                format!(
                    "Returned from #{} (no new messages)",
                    summary.agent_id.as_str()
                ),
                None,
            );
            return;
        }

        let mut context = String::new();
        let _ = writeln!(
            context,
            "Context from #{} (cwd: {})",
            summary.agent_id.as_str(),
            summary.cwd.display()
        );

        for item in capture.user_inputs {
            if let codex_protocol::user_input::UserInput::Text { text } = item {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    let _ = writeln!(context, "You → {trimmed}");
                }
            }
        }

        for message in capture.agent_outputs {
            let trimmed = message.trim();
            if !trimmed.is_empty() {
                let _ = writeln!(context, "{} → {trimmed}", summary.agent_id.as_str());
            }
        }

        let context = context.trim().to_string();
        if context.is_empty() {
            return;
        }

        self.pending_delegate_context.push(context.clone());
        self.add_to_history(history_cell::new_info_event(
            format!("Returned from #{}", summary.agent_id.as_str()),
            Some("Queued delegate context for next prompt.".to_string()),
        ));
        self.add_to_history(history_cell::new_info_event(context, None));
    }

    pub(crate) fn open_feedback_note(
        &mut self,
        category: crate::app_event::FeedbackCategory,
        include_logs: bool,
    ) {
        // Build a fresh snapshot at the time of opening the note overlay.
        let snapshot = self.feedback.snapshot(self.conversation_id);
        let rollout = if include_logs {
            self.current_rollout_path.clone()
        } else {
            None
        };
        let view = crate::bottom_pane::FeedbackNoteView::new(
            category,
            snapshot,
            rollout,
            self.app_event_tx.clone(),
            include_logs,
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    pub(crate) fn open_feedback_consent(&mut self, category: crate::app_event::FeedbackCategory) {
        let params = crate::bottom_pane::feedback_upload_consent_params(
            self.app_event_tx.clone(),
            category,
            self.current_rollout_path.clone(),
        );
        self.bottom_pane.show_selection_view(params);
        self.request_redraw();
    }

    fn on_agent_message(&mut self, message: String) {
        // If we have a stream_controller, then the final agent message is redundant and will be a
        // duplicate of what has already been streamed.
        if self.stream_controller.is_none() {
            self.handle_streaming_delta(message);
        }
        self.flush_answer_stream_with_separator();
        self.handle_stream_finished();
        self.request_redraw();
    }

    fn on_agent_message_delta(&mut self, delta: String) {
        self.handle_streaming_delta(delta);
    }

    fn on_agent_reasoning_delta(&mut self, delta: String) {
        // For reasoning deltas, do not stream to history. Accumulate the
        // current reasoning block and extract the first bold element
        // (between **/**) as the chunk header. Show this header as status.
        self.reasoning_buffer.push_str(&delta);

        if let Some(header) = extract_first_bold(&self.reasoning_buffer) {
            // Update the shimmer header to the extracted reasoning chunk header.
            self.set_status_header(header);
        } else {
            // Fallback while we don't yet have a bold header: leave existing header as-is.
        }
        self.request_redraw();
    }

    fn on_agent_reasoning_final(&mut self) {
        // At the end of a reasoning block, record transcript-only content.
        self.full_reasoning_buffer.push_str(&self.reasoning_buffer);
        if !self.full_reasoning_buffer.is_empty() {
            let cell = history_cell::new_reasoning_summary_block(
                self.full_reasoning_buffer.clone(),
                &self.config,
            );
            self.add_boxed_history(cell);
        }
        self.reasoning_buffer.clear();
        self.full_reasoning_buffer.clear();
        self.request_redraw();
    }

    fn on_reasoning_section_break(&mut self) {
        // Start a new reasoning block for header extraction and accumulate transcript.
        self.full_reasoning_buffer.push_str(&self.reasoning_buffer);
        self.full_reasoning_buffer.push_str("\n\n");
        self.reasoning_buffer.clear();
    }

    // Raw reasoning uses the same flow as summarized reasoning

    fn on_task_started(&mut self) {
        self.bottom_pane.clear_ctrl_c_quit_hint();
        self.queued_turn_pending_start = false;
        self.bottom_pane.set_task_running(true);
        self.retry_status_header = None;
        self.bottom_pane.set_interrupt_hint_visible(true);
        self.set_status_header(String::from("Working"));
        self.full_reasoning_buffer.clear();
        self.reasoning_buffer.clear();
        self.request_redraw();
    }

    fn on_task_complete(&mut self, last_agent_message: Option<String>) {
        // If a stream is currently active, finalize it.
        self.flush_answer_stream_with_separator();
        // Mark task stopped and request redraw now that all content is in history.
        self.queued_turn_pending_start = false;
        self.bottom_pane.set_task_running(false);
        self.running_commands.clear();
        self.suppressed_exec_calls.clear();
        self.last_unified_wait = None;
        self.request_redraw();

        if self.delegate_context.is_some()
            && let Some(message) = last_agent_message.as_ref()
            && !message.trim().is_empty()
        {
            self.delegate_agent_frames.push(message.clone());
        }

        let notification_response = last_agent_message.unwrap_or_default();
        // If there is a queued user message, send exactly one now to begin the next turn.
        // Continue batch image processing if active
        self.on_turn_complete_for_batch();

        self.on_task_complete_for_ralph_loop(&notification_response);

        self.maybe_send_next_queued_input();
        // Emit a notification when the turn completes (suppressed if focused).
        self.notify(Notification::AgentTurnComplete {
            response: notification_response,
        });

        self.maybe_show_pending_rate_limit_prompt();
    }

    fn on_task_complete_for_ralph_loop(&mut self, last_agent_message: &str) {
        let Some(mut state) = self.ralph_loop_state.take() else {
            return;
        };

        let completion_detected =
            check_completion_promise(last_agent_message, &state.completion_promise);
        let max_reached = state.max_iterations > 0 && state.iteration >= state.max_iterations;

        if !completion_detected && !max_reached {
            state.next_iteration(truncate_string(last_agent_message, 200), false);

            if let Err(err) = save_ralph_state_file(&self.config.cwd, &state) {
                tracing::warn!("failed to save ralph state file: {err}");
            }

            let max_iterations_label = if state.max_iterations == 0 {
                "unlimited".to_string()
            } else {
                state.max_iterations.to_string()
            };
            let completion_promise = state.completion_promise.clone();
            let iteration = state.iteration;
            let delay_seconds = state.delay_seconds;

            // Check if we need to delay before the next iteration
            if delay_seconds > 0 {
                let message = format!(
                    "🔄 Ralph iteration {iteration}/{max_iterations_label} | Waiting {delay_seconds}s before next iteration... | To stop: output <promise>{completion_promise}</promise> (ONLY when TRUE)",
                );
                self.add_to_history(history_cell::new_info_event(message, None));

                // Store state and schedule delayed continuation
                self.ralph_loop_state = Some(state);

                // Schedule the delayed continuation via AppEvent
                let app_event_tx = self.app_event_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(delay_seconds)).await;
                    app_event_tx.send(crate::app_event::AppEvent::RalphLoopDelayedContinue);
                });
            } else {
                let message = format!(
                    "🔄 Ralph iteration {iteration}/{max_iterations_label} | To stop: output <promise>{completion_promise}</promise> (ONLY when TRUE)",
                );
                self.add_to_history(history_cell::new_info_event(message, None));

                // Re-inject the SAME original prompt (Ralph technique).
                self.queued_user_messages
                    .push_front(state.original_prompt.clone().into());
                self.refresh_queued_user_messages();

                self.ralph_loop_state = Some(state);
            }
            return;
        }

        let duration_seconds = calculate_duration_seconds(&state.started_at);
        let completion_reason = if completion_detected {
            format!(
                "completion promise detected (<promise>{}</promise>)",
                state.completion_promise.as_str()
            )
        } else {
            format!("max iterations reached ({})", state.max_iterations)
        };
        let msg = format!(
            "✅ Ralph Loop completed: {completion_reason} ({iterations} iteration(s), {duration_seconds:.2}s).",
            iterations = state.iteration,
        );
        self.add_to_history(history_cell::new_info_event(msg, None));

        if let Err(err) = cleanup_ralph_state_file(&self.config.cwd) {
            tracing::warn!("failed to cleanup ralph state file: {err}");
        }

        self.ralph_loop_state = None;
    }

    pub(crate) fn set_token_info(&mut self, info: Option<TokenUsageInfo>) {
        match info {
            Some(info) => self.apply_token_info(info),
            None => {
                self.bottom_pane.set_context_window(None, None);
                self.token_info = None;
            }
        }
    }

    fn apply_token_info(&mut self, info: TokenUsageInfo) {
        let percent = self.context_remaining_percent(&info);
        let used_tokens = self.context_used_tokens(&info, percent.is_some());
        self.bottom_pane.set_context_window(percent, used_tokens);
        self.token_info = Some(info);
    }

    fn context_remaining_percent(&self, info: &TokenUsageInfo) -> Option<i64> {
        info.model_context_window
            .or(self.model_family.context_window)
            .map(|window| {
                info.last_token_usage
                    .percent_of_context_window_remaining(window)
            })
    }

    fn context_used_tokens(&self, info: &TokenUsageInfo, percent_known: bool) -> Option<i64> {
        if percent_known {
            return None;
        }

        Some(info.total_token_usage.tokens_in_context_window())
    }

    fn restore_pre_review_token_info(&mut self) {
        if let Some(saved) = self.pre_review_token_info.take() {
            match saved {
                Some(info) => self.apply_token_info(info),
                None => {
                    self.bottom_pane.set_context_window(None, None);
                    self.token_info = None;
                }
            }
        }
    }

    pub(crate) fn on_rate_limit_snapshot(&mut self, snapshot: Option<RateLimitSnapshot>) {
        if let Some(mut snapshot) = snapshot {
            if snapshot.credits.is_none() {
                snapshot.credits = self
                    .rate_limit_snapshot
                    .as_ref()
                    .and_then(|display| display.credits.as_ref())
                    .map(|credits| CreditsSnapshot {
                        has_credits: credits.has_credits,
                        unlimited: credits.unlimited,
                        balance: credits.balance.clone(),
                    });
            }

            self.plan_type = snapshot.plan_type.or(self.plan_type);
            let warnings = self.rate_limit_warnings.take_warnings(
                snapshot
                    .secondary
                    .as_ref()
                    .map(|window| window.used_percent),
                snapshot
                    .secondary
                    .as_ref()
                    .and_then(|window| window.window_minutes),
                snapshot.primary.as_ref().map(|window| window.used_percent),
                snapshot
                    .primary
                    .as_ref()
                    .and_then(|window| window.window_minutes),
            );

            let high_usage = snapshot
                .secondary
                .as_ref()
                .map(|w| w.used_percent >= RATE_LIMIT_SWITCH_PROMPT_THRESHOLD)
                .unwrap_or(false)
                || snapshot
                    .primary
                    .as_ref()
                    .map(|w| w.used_percent >= RATE_LIMIT_SWITCH_PROMPT_THRESHOLD)
                    .unwrap_or(false);

            if high_usage
                && !self.rate_limit_switch_prompt_hidden()
                && self.model_family.get_model_slug() != NUDGE_MODEL_SLUG
                && !matches!(
                    self.rate_limit_switch_prompt,
                    RateLimitSwitchPromptState::Shown
                )
            {
                self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Pending;
            }

            let display = crate::status::rate_limit_snapshot_display(&snapshot, Local::now());
            self.rate_limit_snapshot = Some(display);

            if !warnings.is_empty() {
                for warning in warnings {
                    self.add_to_history(history_cell::new_warning_event(warning));
                }
                self.request_redraw();
            }
        } else {
            self.rate_limit_snapshot = None;
        }
    }
    /// Finalize any active exec as failed and stop/clear running UI state.
    fn finalize_turn(&mut self) {
        // Ensure any spinner is replaced by a red ✗ and flushed into history.
        self.finalize_active_cell_as_failed();
        // Reset running state and clear streaming buffers.
        self.queued_turn_pending_start = false;
        self.bottom_pane.set_task_running(false);
        self.running_commands.clear();
        self.suppressed_exec_calls.clear();
        self.last_unified_wait = None;
        self.stream_controller = None;
        self.maybe_show_pending_rate_limit_prompt();
    }

    fn on_error(&mut self, message: String) {
        self.finalize_turn();
        self.add_to_history(history_cell::new_error_event(message));
        self.request_redraw();

        // After an error ends the turn, try sending the next queued input.
        self.maybe_send_next_queued_input();
    }

    fn on_warning(&mut self, message: impl Into<String>) {
        self.add_to_history(history_cell::new_warning_event(message.into()));
        self.request_redraw();
    }

    fn on_mcp_startup_update(&mut self, ev: McpStartupUpdateEvent) {
        let mut status = self.mcp_startup_status.take().unwrap_or_default();
        if let McpStartupStatus::Failed { error } = &ev.status {
            self.on_warning(error);
        }
        status.insert(ev.server, ev.status);
        self.mcp_startup_status = Some(status);
        self.bottom_pane.set_task_running(true);
        if let Some(current) = &self.mcp_startup_status {
            let total = current.len();
            let mut starting: Vec<_> = current
                .iter()
                .filter_map(|(name, state)| {
                    if matches!(state, McpStartupStatus::Starting) {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect();
            starting.sort();
            if let Some(first) = starting.first() {
                let completed = total.saturating_sub(starting.len());
                let max_to_show = 3;
                let mut to_show: Vec<String> = starting
                    .iter()
                    .take(max_to_show)
                    .map(ToString::to_string)
                    .collect();
                if starting.len() > max_to_show {
                    to_show.push("…".to_string());
                }
                let header = if total > 1 {
                    format!(
                        "Starting MCP servers ({completed}/{total}): {}",
                        to_show.join(", ")
                    )
                } else {
                    format!("Booting MCP server: {first}")
                };
                self.set_status_header(header);
            }
        }
        self.request_redraw();
    }

    fn on_mcp_startup_complete(&mut self, ev: McpStartupCompleteEvent) {
        let mut parts = Vec::new();
        if !ev.failed.is_empty() {
            let failed_servers: Vec<_> = ev.failed.iter().map(|f| f.server.clone()).collect();
            parts.push(format!("failed: {}", failed_servers.join(", ")));
        }
        if !ev.cancelled.is_empty() {
            self.on_warning(format!(
                "MCP startup interrupted. The following servers were not initialized: {}",
                ev.cancelled.join(", ")
            ));
        }
        if !parts.is_empty() {
            self.on_warning(format!("MCP startup incomplete ({})", parts.join("; ")));
        }

        self.mcp_startup_status = None;
        self.bottom_pane.set_task_running(false);
        self.maybe_send_next_queued_input();
        self.request_redraw();
    }

    /// Handle a turn aborted due to user interrupt (Esc).
    /// When there are queued user messages, restore them into the composer
    /// separated by newlines rather than auto‑submitting the next one.
    fn on_interrupted_turn(&mut self, reason: TurnAbortReason) {
        // Finalize, log a gentle prompt, and clear running state.
        self.finalize_turn();

        if reason != TurnAbortReason::ReviewEnded {
            self.add_to_history(history_cell::new_error_event(
                "Conversation interrupted - tell the model what to do differently. Something went wrong? Hit `/feedback` to report the issue.".to_owned(),
            ));
        }

        // If any messages were queued during the task, restore them into the composer.
        if !self.queued_user_messages.is_empty() {
            let queued_text = self
                .queued_user_messages
                .iter()
                .map(|m| m.text.clone())
                .collect::<Vec<_>>()
                .join("\n");
            let existing_text = self.bottom_pane.composer_text();
            let combined = if existing_text.is_empty() {
                queued_text
            } else if queued_text.is_empty() {
                existing_text
            } else {
                format!("{queued_text}\n{existing_text}")
            };
            self.bottom_pane.set_composer_text(combined);
            // Clear the queue and update the status indicator list.
            self.queued_user_messages.clear();
            self.refresh_queued_user_messages();
        }

        self.request_redraw();
    }

    fn on_plan_update(&mut self, update: UpdatePlanArgs) {
        self.add_to_history(history_cell::new_plan_update(update));
    }

    fn on_exec_approval_request(&mut self, id: String, ev: ExecApprovalRequestEvent) {
        let id2 = id.clone();
        let ev2 = ev.clone();
        self.defer_or_handle(
            |q| q.push_exec_approval(id, ev),
            |s| s.handle_exec_approval_now(id2, ev2),
        );
    }

    fn on_apply_patch_approval_request(&mut self, id: String, ev: ApplyPatchApprovalRequestEvent) {
        let id2 = id.clone();
        let ev2 = ev.clone();
        self.defer_or_handle(
            |q| q.push_apply_patch_approval(id, ev),
            |s| s.handle_apply_patch_approval_now(id2, ev2),
        );
    }

    fn on_elicitation_request(&mut self, ev: ElicitationRequestEvent) {
        let ev2 = ev.clone();
        self.defer_or_handle(
            |q| q.push_elicitation(ev),
            |s| s.handle_elicitation_request_now(ev2),
        );
    }

    fn on_exec_command_begin(&mut self, ev: ExecCommandBeginEvent) {
        self.flush_answer_stream_with_separator();
        let ev2 = ev.clone();
        self.defer_or_handle(|q| q.push_exec_begin(ev), |s| s.handle_exec_begin_now(ev2));
    }

    fn on_exec_command_output_delta(
        &mut self,
        _ev: codex_core::protocol::ExecCommandOutputDeltaEvent,
    ) {
        // TODO: Handle streaming exec output if/when implemented
    }

    fn on_terminal_interaction(&mut self, _ev: TerminalInteractionEvent) {
        // TODO: Handle once design is ready.
    }

    fn on_patch_apply_begin(&mut self, event: PatchApplyBeginEvent) {
        self.add_to_history(history_cell::new_patch_event(
            event.changes,
            &self.config.cwd,
        ));
    }

    fn on_view_image_tool_call(&mut self, event: ViewImageToolCallEvent) {
        self.flush_answer_stream_with_separator();
        self.add_to_history(history_cell::new_view_image_tool_call(
            event.path,
            &self.config.cwd,
        ));
        self.request_redraw();
    }

    fn on_patch_apply_end(&mut self, event: codex_core::protocol::PatchApplyEndEvent) {
        let ev2 = event.clone();
        self.defer_or_handle(
            |q| q.push_patch_end(event),
            |s| s.handle_patch_apply_end_now(ev2),
        );
    }

    fn on_exec_command_end(&mut self, ev: ExecCommandEndEvent) {
        let ev2 = ev.clone();
        self.defer_or_handle(|q| q.push_exec_end(ev), |s| s.handle_exec_end_now(ev2));
    }

    fn on_mcp_tool_call_begin(&mut self, ev: McpToolCallBeginEvent) {
        let ev2 = ev.clone();
        self.defer_or_handle(|q| q.push_mcp_begin(ev), |s| s.handle_mcp_begin_now(ev2));
    }

    fn on_mcp_tool_call_end(&mut self, ev: McpToolCallEndEvent) {
        let ev2 = ev.clone();
        self.defer_or_handle(|q| q.push_mcp_end(ev), |s| s.handle_mcp_end_now(ev2));
    }

    fn on_web_search_begin(&mut self, _ev: WebSearchBeginEvent) {
        self.flush_answer_stream_with_separator();
    }

    fn on_web_search_end(&mut self, ev: WebSearchEndEvent) {
        self.flush_answer_stream_with_separator();
        self.add_to_history(history_cell::new_web_search_call(ev.query));
    }

    fn on_get_history_entry_response(
        &mut self,
        event: codex_core::protocol::GetHistoryEntryResponseEvent,
    ) {
        let codex_core::protocol::GetHistoryEntryResponseEvent {
            offset,
            log_id,
            entry,
        } = event;
        self.bottom_pane
            .on_history_entry_response(log_id, offset, entry.map(|e| e.text));
    }

    fn on_shutdown_complete(&mut self) {
        self.request_exit();
    }

    fn on_turn_diff(&mut self, unified_diff: String) {
        debug!("TurnDiffEvent: {unified_diff}");
    }

    fn on_deprecation_notice(&mut self, event: DeprecationNoticeEvent) {
        let DeprecationNoticeEvent { summary, details } = event;
        self.add_to_history(history_cell::new_deprecation_notice(summary, details));
        self.request_redraw();
    }

    fn on_background_event(&mut self, message: String) {
        debug!("BackgroundEvent: {message}");
        self.bottom_pane.ensure_status_indicator();
        self.bottom_pane.set_interrupt_hint_visible(true);
        self.set_status_header(message);
    }

    fn on_ralph_loop_continue(&mut self, event: RalphLoopContinueEvent) {
        let RalphLoopContinueEvent {
            iteration,
            max_iterations,
            reason,
        } = event;
        let hint = if reason.trim().is_empty() {
            None
        } else {
            Some(reason)
        };
        self.add_info_message(
            format!("Ralph Loop continuing ({iteration}/{max_iterations})"),
            hint,
        );
    }

    fn on_ralph_loop_status(&mut self, event: RalphLoopStatusEvent) {
        self.add_info_message(event.message, None);
    }

    fn on_ralph_loop_complete(&mut self, event: RalphLoopCompleteEvent) {
        let RalphLoopCompleteEvent {
            total_iterations,
            completion_reason,
            duration_seconds,
        } = event;
        let reason = match completion_reason {
            RalphCompletionReason::PromiseDetected => "completion promise detected",
            RalphCompletionReason::MaxIterations => "max iterations reached",
            RalphCompletionReason::UserInterrupt => "cancelled by user",
            RalphCompletionReason::FatalError => "fatal error",
        };
        let message = format!(
            "Ralph Loop completed: {reason} ({total_iterations} iterations, {duration_seconds:.2}s)",
        );
        if matches!(completion_reason, RalphCompletionReason::FatalError) {
            self.add_error_message(message);
        } else {
            self.add_info_message(message, None);
        }
    }

    fn on_undo_started(&mut self, event: UndoStartedEvent) {
        self.bottom_pane.ensure_status_indicator();
        self.bottom_pane.set_interrupt_hint_visible(false);
        let message = event
            .message
            .unwrap_or_else(|| "Undo in progress...".to_string());
        self.set_status_header(message);
    }

    fn on_undo_completed(&mut self, event: UndoCompletedEvent) {
        let UndoCompletedEvent { success, message } = event;
        self.bottom_pane.hide_status_indicator();
        let message = message.unwrap_or_else(|| {
            if success {
                "Undo completed successfully.".to_string()
            } else {
                "Undo failed.".to_string()
            }
        });
        if success {
            self.add_info_message(message, None);
        } else {
            self.add_error_message(message);
        }
    }

    fn on_stream_error(&mut self, message: String) {
        if self.retry_status_header.is_none() {
            self.retry_status_header = Some(self.current_status_header.clone());
        }
        self.set_status_header(message);
    }

    /// Periodic tick to commit at most one queued line to history with a small delay,
    /// animating the output.
    pub(crate) fn on_commit_tick(&mut self) {
        if let Some(controller) = self.stream_controller.as_mut() {
            let (cell, is_idle) = controller.on_commit_tick();
            if let Some(cell) = cell {
                self.bottom_pane.hide_status_indicator();
                self.add_boxed_history(cell);
            }
            if is_idle {
                self.app_event_tx.send(AppEvent::StopCommitAnimation);
            }
        }
    }

    fn flush_interrupt_queue(&mut self) {
        let mut mgr = std::mem::take(&mut self.interrupts);
        mgr.flush_all(self);
        self.interrupts = mgr;
    }

    #[inline]
    fn defer_or_handle(
        &mut self,
        push: impl FnOnce(&mut InterruptManager),
        handle: impl FnOnce(&mut Self),
    ) {
        // Preserve deterministic FIFO across queued interrupts: once anything
        // is queued due to an active write cycle, continue queueing until the
        // queue is flushed to avoid reordering (e.g., ExecEnd before ExecBegin).
        if self.stream_controller.is_some() || !self.interrupts.is_empty() {
            push(&mut self.interrupts);
        } else {
            handle(self);
        }
    }

    fn handle_stream_finished(&mut self) {
        if self.task_complete_pending {
            self.bottom_pane.hide_status_indicator();
            self.task_complete_pending = false;
        }
        // A completed stream indicates non-exec content was just inserted.
        self.flush_interrupt_queue();
    }

    #[inline]
    fn handle_streaming_delta(&mut self, delta: String) {
        // Before streaming agent content, flush any active exec cell group.
        self.flush_active_cell();
        if let Some(header) = self.retry_status_header.take() {
            self.set_status_header(header);
        }

        if self.stream_controller.is_none() {
            if self.needs_final_message_separator {
                let elapsed_seconds = self
                    .bottom_pane
                    .status_widget()
                    .map(super::status_indicator_widget::StatusIndicatorWidget::elapsed_seconds);
                self.add_to_history(history_cell::FinalMessageSeparator::new(elapsed_seconds));
                self.needs_final_message_separator = false;
            }
            self.stream_controller = Some(StreamController::new(
                self.last_rendered_width.get().map(|w| w.saturating_sub(2)),
            ));
        }
        if let Some(controller) = self.stream_controller.as_mut()
            && controller.push(&delta)
        {
            self.app_event_tx.send(AppEvent::StartCommitAnimation);
        }
        self.request_redraw();
    }

    pub(crate) fn handle_exec_end_now(&mut self, ev: ExecCommandEndEvent) {
        let ExecCommandEndEvent {
            call_id,
            command: end_command,
            parsed_cmd: end_parsed,
            source: end_source,
            exit_code,
            formatted_output,
            aggregated_output,
            duration,
            ..
        } = ev;

        let running = self.running_commands.remove(&call_id);
        if self.suppressed_exec_calls.remove(&call_id) {
            return;
        }
        let (command, parsed, source) = match running {
            Some(rc) => (rc.command, rc.parsed_cmd, rc.source),
            None => (end_command, end_parsed, end_source),
        };
        let is_unified_exec_interaction =
            matches!(source, ExecCommandSource::UnifiedExecInteraction);

        // Sidebar status: Success/Failure summary
        // 已移除事件发送，改为在 sidebar_status() 中实时计算

        let needs_new = self
            .active_cell
            .as_ref()
            .map(|cell| cell.as_any().downcast_ref::<ExecCell>().is_none())
            .unwrap_or(true);
        if needs_new {
            self.flush_active_cell();
            self.active_cell = Some(Box::new(new_active_exec_command(
                call_id.clone(),
                command,
                parsed,
                source,
                None,
                self.config.animations,
            )));
        }

        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<ExecCell>())
        {
            let output = if is_unified_exec_interaction {
                CommandOutput {
                    exit_code,
                    formatted_output: String::new(),
                    aggregated_output: String::new(),
                }
            } else {
                CommandOutput {
                    exit_code,
                    formatted_output,
                    aggregated_output,
                }
            };
            cell.complete_call(&call_id, output, duration);
            if cell.should_flush() {
                self.flush_active_cell();
            }
        }
    }

    pub(crate) fn handle_patch_apply_end_now(
        &mut self,
        event: codex_core::protocol::PatchApplyEndEvent,
    ) {
        // If the patch was successful, just let the "Edited" block stand.
        // Otherwise, add a failure block.
        if !event.success {
            self.add_to_history(history_cell::new_patch_apply_failure(event.stderr));
        }
    }

    pub(crate) fn handle_exec_approval_now(&mut self, id: String, ev: ExecApprovalRequestEvent) {
        self.flush_answer_stream_with_separator();
        let command = shlex::try_join(ev.command.iter().map(String::as_str))
            .unwrap_or_else(|_| ev.command.join(" "));
        self.notify(Notification::ExecApprovalRequested { command });

        let request = ApprovalRequest::Exec {
            id,
            command: ev.command,
            reason: ev.reason,
            proposed_execpolicy_amendment: ev.proposed_execpolicy_amendment,
        };
        self.bottom_pane
            .push_approval_request(request, &self.config.features);
        self.request_redraw();

        // Sidebar status: Waiting for approval
        // 已移除事件发送，改为在 sidebar_status() 中实时计算
    }

    pub(crate) fn handle_apply_patch_approval_now(
        &mut self,
        id: String,
        ev: ApplyPatchApprovalRequestEvent,
    ) {
        self.flush_answer_stream_with_separator();

        let request = ApprovalRequest::ApplyPatch {
            id,
            reason: ev.reason,
            changes: ev.changes.clone(),
            cwd: self.config.cwd.clone(),
        };
        self.bottom_pane
            .push_approval_request(request, &self.config.features);
        self.request_redraw();
        self.notify(Notification::EditApprovalRequested {
            cwd: self.config.cwd.clone(),
            changes: ev.changes.keys().cloned().collect(),
        });
    }

    pub(crate) fn handle_elicitation_request_now(&mut self, ev: ElicitationRequestEvent) {
        self.flush_answer_stream_with_separator();

        self.notify(Notification::ElicitationRequested {
            server_name: ev.server_name.clone(),
        });

        let request = ApprovalRequest::McpElicitation {
            server_name: ev.server_name,
            request_id: ev.id,
            message: ev.message,
        };
        self.bottom_pane
            .push_approval_request(request, &self.config.features);
        self.request_redraw();
    }

    pub(crate) fn handle_exec_begin_now(&mut self, ev: ExecCommandBeginEvent) {
        // Ensure the status indicator is visible while the command runs.
        self.running_commands.insert(
            ev.call_id.clone(),
            RunningCommand {
                command: ev.command.clone(),
                parsed_cmd: ev.parsed_cmd.clone(),
                source: ev.source,
            },
        );
        let is_wait_interaction = matches!(ev.source, ExecCommandSource::UnifiedExecInteraction)
            && ev
                .interaction_input
                .as_deref()
                .map(str::is_empty)
                .unwrap_or(true);
        let command_display = ev.command.join(" ");
        let should_suppress_unified_wait = is_wait_interaction
            && self
                .last_unified_wait
                .as_ref()
                .is_some_and(|wait| wait.is_duplicate(&command_display));
        if is_wait_interaction {
            self.last_unified_wait = Some(UnifiedExecWaitState::new(command_display));
        } else {
            self.last_unified_wait = None;
        }
        if should_suppress_unified_wait {
            self.suppressed_exec_calls.insert(ev.call_id);
            return;
        }
        let interaction_input = ev.interaction_input.clone();
        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<ExecCell>())
            && let Some(new_exec) = cell.with_added_call(
                ev.call_id.clone(),
                ev.command.clone(),
                ev.parsed_cmd.clone(),
                ev.source,
                interaction_input.clone(),
            )
        {
            *cell = new_exec;
        } else {
            self.flush_active_cell();

            self.active_cell = Some(Box::new(new_active_exec_command(
                ev.call_id.clone(),
                ev.command.clone(),
                ev.parsed_cmd,
                ev.source,
                interaction_input,
                self.config.animations,
            )));
        }

        // Sidebar status: Running
        // 已移除事件发送，改为在 sidebar_status() 中实时计算

        self.request_redraw();
    }

    pub(crate) fn handle_mcp_begin_now(&mut self, ev: McpToolCallBeginEvent) {
        self.flush_answer_stream_with_separator();
        self.flush_active_cell();
        self.active_cell = Some(Box::new(history_cell::new_active_mcp_tool_call(
            ev.call_id,
            ev.invocation,
            self.config.animations,
        )));
        self.request_redraw();
    }
    pub(crate) fn handle_mcp_end_now(&mut self, ev: McpToolCallEndEvent) {
        self.flush_answer_stream_with_separator();

        let McpToolCallEndEvent {
            call_id,
            invocation,
            duration,
            result,
        } = ev;

        let extra_cell = match self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<McpToolCallCell>())
        {
            Some(cell) if cell.call_id() == call_id => cell.complete(duration, result),
            _ => {
                self.flush_active_cell();
                let mut cell = history_cell::new_active_mcp_tool_call(
                    call_id,
                    invocation,
                    self.config.animations,
                );
                let extra_cell = cell.complete(duration, result);
                self.active_cell = Some(Box::new(cell));
                extra_cell
            }
        };

        self.flush_active_cell();
        if let Some(extra) = extra_cell {
            self.add_boxed_history(extra);
        }
    }

    pub(crate) fn new(
        common: ChatWidgetInit,
        conversation_manager: Arc<ConversationManager>,
    ) -> Self {
        let ChatWidgetInit {
            config,
            frame_requester,
            app_event_tx,
            initial_prompt,
            initial_images,
            enhanced_keys_supported,
            auth_manager,
            models_manager,
            feedback,
            is_first_run,
            model_family,
        } = common;
        let model_slug = model_family.get_model_slug().to_string();
        let mut config = config;
        config.model = Some(model_slug.clone());
        let mut rng = rand::rng();
        let placeholder = EXAMPLE_PROMPTS[rng.random_range(0..EXAMPLE_PROMPTS.len())].to_string();
        let codex_op_tx = spawn_agent(config.clone(), app_event_tx.clone(), conversation_manager);

        let mut widget = Self {
            app_event_tx: app_event_tx.clone(),
            frame_requester: frame_requester.clone(),
            codex_op_tx,
            bottom_pane: BottomPane::new(BottomPaneParams {
                frame_requester,
                app_event_tx,
                has_input_focus: true,
                enhanced_keys_supported,
                placeholder_text: placeholder,
                disable_paste_burst: config.disable_paste_burst,
                animations_enabled: config.animations,
                skills: None,
            }),
            active_cell: None,
            config,
            model_family,
            auth_manager,
            models_manager,
            session_header: SessionHeader::new(model_slug),
            initial_user_message: create_initial_user_message(
                initial_prompt.unwrap_or_default(),
                initial_images,
            ),
            defer_initial_message_for_alias_input: false,
            token_info: None,
            rate_limit_snapshot: None,
            plan_type: None,
            rate_limit_warnings: RateLimitWarningState::default(),
            rate_limit_switch_prompt: RateLimitSwitchPromptState::default(),
            rate_limit_poller: None,
            stream_controller: None,
            running_commands: HashMap::new(),
            suppressed_exec_calls: HashSet::new(),
            last_unified_wait: None,
            task_complete_pending: false,
            mcp_startup_status: None,
            interrupts: InterruptManager::new(),
            reasoning_buffer: String::new(),
            full_reasoning_buffer: String::new(),
            current_status_header: String::from("Working"),
            retry_status_header: None,
            conversation_id: None,
            queued_user_messages: VecDeque::new(),
            queued_turn_pending_start: false,
            last_user_message: None,
            ralph_loop_state: None,
            show_welcome_banner: is_first_run,
            suppress_session_configured_redraw: false,
            pending_notification: None,
            is_review_mode: false,
            pre_review_token_info: None,
            needs_final_message_separator: false,
            delegate_run: None,
            delegate_runs_with_stream: HashSet::new(),
            delegate_status_owner: None,
            delegate_previous_status_header: None,
            delegate_context: None,
            delegate_user_frames: Vec::new(),
            delegate_agent_frames: Vec::new(),
            pending_delegate_context: Vec::new(),
            last_rendered_width: std::cell::Cell::new(None),
            feedback,
            current_rollout_path: None,
            next_generated_image_index: 0,
            last_generated_image_path: None,
            ref_images: RefImageManager::new(),
            batch_image_state: None,
            pending_pdf_update: None,
        };

        widget.prefetch_rate_limits();

        widget
    }

    /// Create a ChatWidget attached to an existing conversation (e.g., a fork).
    pub(crate) fn new_from_existing(
        common: ChatWidgetInit,
        conversation_id: String,
        conversation: std::sync::Arc<codex_core::CodexConversation>,
        session_configured: codex_core::protocol::SessionConfiguredEvent,
    ) -> Self {
        let ChatWidgetInit {
            config,
            frame_requester,
            app_event_tx,
            initial_prompt,
            initial_images,
            enhanced_keys_supported,
            auth_manager,
            models_manager,
            feedback,
            model_family,
            ..
        } = common;
        let mut rng = rand::rng();
        let placeholder = EXAMPLE_PROMPTS[rng.random_range(0..EXAMPLE_PROMPTS.len())].to_string();

        let model_slug = model_family.get_model_slug().to_string();

        let codex_op_tx = spawn_agent_from_existing(
            conversation_id.clone(),
            conversation,
            session_configured,
            app_event_tx.clone(),
        );

        let mut widget = Self {
            app_event_tx: app_event_tx.clone(),
            frame_requester: frame_requester.clone(),
            codex_op_tx,
            bottom_pane: BottomPane::new(BottomPaneParams {
                frame_requester,
                app_event_tx,
                has_input_focus: true,
                enhanced_keys_supported,
                placeholder_text: placeholder,
                disable_paste_burst: config.disable_paste_burst,
                animations_enabled: config.animations,
                skills: None,
            }),
            active_cell: None,
            config,
            model_family,
            auth_manager,
            models_manager,
            session_header: SessionHeader::new(model_slug),
            initial_user_message: create_initial_user_message(
                initial_prompt.unwrap_or_default(),
                initial_images,
            ),
            defer_initial_message_for_alias_input: false,
            token_info: None,
            rate_limit_snapshot: None,
            plan_type: None,
            rate_limit_warnings: RateLimitWarningState::default(),
            rate_limit_switch_prompt: RateLimitSwitchPromptState::default(),
            rate_limit_poller: None,
            stream_controller: None,
            running_commands: HashMap::new(),
            suppressed_exec_calls: HashSet::new(),
            last_unified_wait: None,
            task_complete_pending: false,
            mcp_startup_status: None,
            interrupts: InterruptManager::new(),
            reasoning_buffer: String::new(),
            full_reasoning_buffer: String::new(),
            current_status_header: String::from("Working"),
            retry_status_header: None,
            conversation_id: ConversationId::from_string(&conversation_id).ok(),
            queued_user_messages: VecDeque::new(),
            queued_turn_pending_start: false,
            last_user_message: None,
            ralph_loop_state: None,
            show_welcome_banner: false,
            suppress_session_configured_redraw: true,
            pending_notification: None,
            is_review_mode: false,
            pre_review_token_info: None,
            needs_final_message_separator: false,
            delegate_run: None,
            delegate_runs_with_stream: HashSet::new(),
            delegate_status_owner: None,
            delegate_previous_status_header: None,
            delegate_context: None,
            delegate_user_frames: Vec::new(),
            delegate_agent_frames: Vec::new(),
            pending_delegate_context: Vec::new(),
            last_rendered_width: std::cell::Cell::new(None),
            feedback,
            current_rollout_path: None,
            next_generated_image_index: 0,
            last_generated_image_path: None,
            ref_images: RefImageManager::new(),
            batch_image_state: None,
            pending_pdf_update: None,
        };

        widget.prefetch_rate_limits();

        widget
    }

    pub(crate) fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                kind: KeyEventKind::Press,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) && c.eq_ignore_ascii_case(&'c') => {
                self.on_ctrl_c();
                return;
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                kind: KeyEventKind::Press,
                ..
            } if modifiers.contains(KeyModifiers::CONTROL) && c.eq_ignore_ascii_case(&'v') => {
                match paste_image_to_temp_png() {
                    Ok((path, info)) => {
                        self.attach_image(
                            path,
                            info.width,
                            info.height,
                            info.encoded_format.label(),
                        );
                    }
                    Err(err) => {
                        tracing::warn!("failed to paste image: {err}");
                        self.add_to_history(history_cell::new_error_event(format!(
                            "Failed to paste image: {err}",
                        )));
                    }
                }
                return;
            }
            other if other.kind == KeyEventKind::Press => {
                self.bottom_pane.clear_ctrl_c_quit_hint();
            }
            _ => {}
        }

        match key_event {
            KeyEvent {
                code: KeyCode::Up,
                modifiers: KeyModifiers::ALT,
                kind: KeyEventKind::Press,
                ..
            } if !self.queued_user_messages.is_empty() => {
                // Prefer the most recently queued item.
                if let Some(user_message) = self.queued_user_messages.pop_back() {
                    self.bottom_pane.set_composer_text(user_message.text);
                    self.refresh_queued_user_messages();
                    self.request_redraw();
                }
            }
            _ => {
                match self.bottom_pane.handle_key_event(key_event) {
                    InputResult::Submitted(text) => {
                        // If a task is running, queue the user input to be sent after the turn completes.
                        let user_message = UserMessage {
                            text,
                            image_paths: self.bottom_pane.take_recent_submission_images(),
                        };
                        self.queue_user_message(user_message);
                    }
                    InputResult::Command(cmd) => {
                        self.dispatch_command(cmd, None);
                    }
                    InputResult::CommandWithArgs(cmd, args) => {
                        self.dispatch_command(cmd, Some(args));
                    }
                    InputResult::None => {}
                }
            }
        }

        if self.defer_initial_message_for_alias_input && !self.bottom_pane.has_active_view() {
            self.defer_initial_message_for_alias_input = false;
            if let Some(user_message) = self.initial_user_message.take() {
                self.submit_user_message(user_message);
            }
        }
    }

    pub(crate) fn attach_image(
        &mut self,
        path: PathBuf,
        width: u32,
        height: u32,
        format_label: &str,
    ) {
        tracing::info!(
            "attach_image path={path:?} width={width} height={height} format={format_label}",
        );
        self.bottom_pane
            .attach_image(path, width, height, format_label);
        self.request_redraw();
    }

    fn dispatch_command(&mut self, cmd: SlashCommand, args: Option<String>) {
        if !cmd.available_during_task() && self.bottom_pane.is_task_running() {
            let message = format!(
                "'/{}' is disabled while a task is in progress.",
                cmd.command()
            );
            self.add_to_history(history_cell::new_error_event(message));
            self.request_redraw();
            return;
        }
        match cmd {
            SlashCommand::Feedback => {
                // Step 1: pick a category (UI built in feedback_view)
                let params =
                    crate::bottom_pane::feedback_selection_params(self.app_event_tx.clone());
                self.bottom_pane.show_selection_view(params);
                self.request_redraw();
            }
            SlashCommand::New => {
                self.app_event_tx.send(AppEvent::NewSession);
            }
            SlashCommand::Resume => {
                self.app_event_tx.send(AppEvent::OpenResumePicker);
            }
            SlashCommand::Init => {
                let init_target = self.config.cwd.join(DEFAULT_PROJECT_DOC_FILENAME);
                if init_target.exists() {
                    let message = format!(
                        "{DEFAULT_PROJECT_DOC_FILENAME} already exists here. Skipping /init to avoid overwriting it."
                    );
                    self.add_info_message(message, None);
                    return;
                }
                const INIT_PROMPT: &str = include_str!("../prompt_for_init_command.md");
                self.submit_user_message(INIT_PROMPT.to_string().into());
            }
            SlashCommand::Tumix => {
                self.handle_tumix_command(args);
            }
            SlashCommand::TumixStop => {
                self.handle_tumix_stop_command(args);
            }
            SlashCommand::Compact => {
                self.clear_token_usage();
                self.app_event_tx.send(AppEvent::CodexOp(Op::Compact));
            }
            SlashCommand::Review => {
                self.open_review_popup();
            }
            SlashCommand::RalphLoop => {
                self.handle_ralph_loop_command(args);
            }
            SlashCommand::CancelRalph => {
                self.handle_cancel_ralph_command();
            }
            SlashCommand::Model => {
                self.open_model_popup();
            }
            SlashCommand::Approvals => {
                self.open_approvals_popup();
            }
            SlashCommand::Quit | SlashCommand::Exit => {
                self.request_exit();
            }
            SlashCommand::Logout => {
                if let Err(e) = codex_core::auth::logout(
                    &self.config.codex_home,
                    self.config.cli_auth_credentials_store_mode,
                ) {
                    tracing::error!("failed to logout: {e}");
                }
                self.request_exit();
            }
            SlashCommand::Undo => {
                self.app_event_tx.send(AppEvent::CodexOp(Op::Undo));
            }
            SlashCommand::Diff => {
                self.add_diff_in_progress();
                let tx = self.app_event_tx.clone();
                tokio::spawn(async move {
                    let text = match get_git_diff().await {
                        Ok((is_git_repo, diff_text)) => {
                            if is_git_repo {
                                diff_text
                            } else {
                                "`/diff` — _not inside a git repository_".to_string()
                            }
                        }
                        Err(e) => format!("Failed to compute diff: {e}"),
                    };
                    tx.send(AppEvent::DiffResult(text));
                });
            }
            SlashCommand::OpenImage => {
                self.open_last_generated_image();
            }
            SlashCommand::RefImage => {
                self.handle_ref_image_command(args);
            }
            SlashCommand::RefImageBatch => {
                self.handle_ref_image_batch_command(args);
            }
            SlashCommand::PdfUpdate => {
                self.handle_pdf_update_command(args);
            }
            SlashCommand::ImageQuality => {
                self.handle_image_quality_command(args);
            }
            SlashCommand::ClearRef => {
                self.ref_images.clear();
                self.app_event_tx
                    .send(AppEvent::CodexOp(Op::ClearReferenceImages));
                self.add_info_message("Reference images cleared.".to_string(), None);
            }
            SlashCommand::Mention => {
                self.insert_str("@");
            }
            SlashCommand::Agent => {
                self.app_event_tx.send(AppEvent::OpenDelegatePicker);
            }
            SlashCommand::Status => {
                self.add_status_output();
            }
            SlashCommand::Mcp => {
                self.add_mcp_output();
            }
            SlashCommand::Rollout => {
                if let Some(path) = self.rollout_path() {
                    self.add_info_message(
                        format!("Current rollout path: {}", path.display()),
                        None,
                    );
                } else {
                    self.add_info_message("Rollout path is not available yet.".to_string(), None);
                }
            }
            SlashCommand::TestApproval => {
                use codex_core::protocol::EventMsg;
                use std::collections::HashMap;

                use codex_core::protocol::ApplyPatchApprovalRequestEvent;
                use codex_core::protocol::FileChange;

                self.app_event_tx.send(AppEvent::CodexEvent(Event {
                    id: "1".to_string(),
                    // msg: EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
                    //     call_id: "1".to_string(),
                    //     command: vec!["git".into(), "apply".into()],
                    //     cwd: self.config.cwd.clone(),
                    //     reason: Some("test".to_string()),
                    // }),
                    msg: EventMsg::ApplyPatchApprovalRequest(ApplyPatchApprovalRequestEvent {
                        call_id: "1".to_string(),
                        turn_id: "turn-1".to_string(),
                        changes: HashMap::from([
                            (
                                PathBuf::from("/tmp/test.txt"),
                                FileChange::Add {
                                    content: "test".to_string(),
                                },
                            ),
                            (
                                PathBuf::from("/tmp/test2.txt"),
                                FileChange::Update {
                                    unified_diff: "+test\n-test2".to_string(),
                                    move_path: None,
                                },
                            ),
                        ]),
                        reason: None,
                        grant_root: Some(PathBuf::from("/tmp")),
                    }),
                }));
            }
        }
    }

    pub(crate) fn handle_paste(&mut self, text: String) {
        self.bottom_pane.handle_paste(text);
    }

    // Returns true if caller should skip rendering this frame (a future frame is scheduled).
    pub(crate) fn handle_paste_burst_tick(&mut self, frame_requester: FrameRequester) -> bool {
        if self.bottom_pane.flush_paste_burst_if_due() {
            // A paste just flushed; request an immediate redraw and skip this frame.
            self.request_redraw();
            true
        } else if self.bottom_pane.is_in_paste_burst() {
            // While capturing a burst, schedule a follow-up tick and skip this frame
            // to avoid redundant renders between ticks.
            frame_requester.schedule_frame_in(
                crate::bottom_pane::ChatComposer::recommended_paste_flush_delay(),
            );
            true
        } else {
            false
        }
    }

    fn flush_active_cell(&mut self) {
        if let Some(active) = self.active_cell.take() {
            self.needs_final_message_separator = true;
            self.app_event_tx.send(AppEvent::InsertHistoryCell(active));
        }
    }

    fn add_to_history(&mut self, cell: impl HistoryCell + 'static) {
        self.add_boxed_history(Box::new(cell));
    }

    fn add_boxed_history(&mut self, cell: Box<dyn HistoryCell>) {
        if !cell.display_lines(u16::MAX).is_empty() {
            // Only break exec grouping if the cell renders visible lines.
            self.flush_active_cell();
            self.needs_final_message_separator = true;
        }
        self.app_event_tx.send(AppEvent::InsertHistoryCell(cell));
    }

    fn queue_user_message(&mut self, user_message: UserMessage) {
        if self.bottom_pane.is_task_running() {
            self.queued_user_messages.push_back(user_message);
            self.refresh_queued_user_messages();
        } else {
            self.submit_user_message(user_message);
        }
    }

    fn submit_user_message(&mut self, user_message: UserMessage) {
        let UserMessage {
            mut text,
            image_paths,
        } = user_message;
        if text.is_empty() && image_paths.is_empty() {
            return;
        }

        let display_text = text.clone();

        if self.delegate_context.is_some()
            && !display_text.trim().is_empty()
            && image_paths.is_empty()
        {
            self.delegate_user_frames
                .push(codex_protocol::user_input::UserInput::Text {
                    text: display_text.clone(),
                });
        }

        // Intercept explicit delegation commands (only support text-only submissions).
        if image_paths.is_empty() && !text.is_empty() && self.try_delegate_shortcut(&text) {
            return;
        }

        if self.delegate_context.is_none()
            && !self.pending_delegate_context.is_empty()
            && !text.trim().is_empty()
        {
            let mut prefix = self.pending_delegate_context.join("\n\n");
            self.pending_delegate_context.clear();
            if !prefix.is_empty() {
                if !prefix.ends_with('\n') {
                    prefix.push('\n');
                }
                prefix.push('\n');
            }
            prefix.push_str(&text);
            text = prefix;
        }

        let mut items: Vec<UserInput> = Vec::new();

        // Special-case: "!cmd" executes a local shell command instead of sending to the model.
        if let Some(stripped) = text.strip_prefix('!') {
            let cmd = stripped.trim();
            if cmd.is_empty() {
                self.app_event_tx.send(AppEvent::InsertHistoryCell(Box::new(
                    history_cell::new_info_event(
                        USER_SHELL_COMMAND_HELP_TITLE.to_string(),
                        Some(USER_SHELL_COMMAND_HELP_HINT.to_string()),
                    ),
                )));
                return;
            }
            self.submit_op(Op::RunUserShellCommand {
                command: cmd.to_string(),
            });
            return;
        }

        if !text.is_empty() {
            items.push(UserInput::Text { text: text.clone() });
        }

        for path in image_paths {
            items.push(UserInput::LocalImage { path });
        }

        if let Err(e) = self.codex_op_tx.send(Op::UserInput { items }) {
            tracing::error!("failed to send message: {e}");
        }

        if !text.is_empty()
            && let Err(e) = self
                .codex_op_tx
                .send(Op::AddToHistory { text: text.clone() })
        {
            tracing::error!("failed to send AddHistory op: {e}");
        }

        if !display_text.trim().is_empty() {
            self.last_user_message = Some(display_text.clone());
        }
        if !display_text.is_empty() {
            self.add_to_history(history_cell::new_user_prompt(display_text));
        }
        self.needs_final_message_separator = false;
    }

    /// Replay a subset of initial events into the UI to seed the transcript when
    /// resuming an existing session. This approximates the live event flow and
    /// is intentionally conservative: only safe-to-replay items are rendered to
    /// avoid triggering side effects. Event ids are passed as `None` to
    /// distinguish replayed events from live ones.
    fn replay_initial_messages(&mut self, events: Vec<EventMsg>) {
        for msg in events {
            if matches!(msg, EventMsg::SessionConfigured(_)) {
                continue;
            }
            // `id: None` indicates a synthetic/fake id coming from replay.
            self.dispatch_event_msg(None, msg, true);
        }
    }

    pub(crate) fn handle_codex_event(&mut self, event: Event) {
        let Event { id, msg } = event;
        self.dispatch_event_msg(Some(id), msg, false);
    }

    /// Dispatch a protocol `EventMsg` to the appropriate handler.
    ///
    /// `id` is `Some` for live events and `None` for replayed events from
    /// `replay_initial_messages()`. Callers should treat `None` as a "fake" id
    /// that must not be used to correlate follow-up actions.
    fn dispatch_event_msg(&mut self, id: Option<String>, msg: EventMsg, from_replay: bool) {
        match msg {
            EventMsg::AgentMessageDelta(_)
            | EventMsg::AgentReasoningDelta(_)
            | EventMsg::ExecCommandOutputDelta(_)
            | EventMsg::TerminalInteraction(_) => {}
            _ => {
                tracing::trace!("handle_codex_event: {:?}", msg);
            }
        }

        match msg {
            EventMsg::SessionConfigured(e) => self.on_session_configured(e),
            EventMsg::AgentMessage(AgentMessageEvent { message }) => self.on_agent_message(message),
            EventMsg::AgentMessageDelta(AgentMessageDeltaEvent { delta }) => {
                self.on_agent_message_delta(delta)
            }
            EventMsg::AgentReasoningDelta(AgentReasoningDeltaEvent { delta })
            | EventMsg::AgentReasoningRawContentDelta(AgentReasoningRawContentDeltaEvent {
                delta,
            }) => self.on_agent_reasoning_delta(delta),
            EventMsg::AgentReasoning(AgentReasoningEvent { .. }) => self.on_agent_reasoning_final(),
            EventMsg::AgentReasoningRawContent(AgentReasoningRawContentEvent { text }) => {
                self.on_agent_reasoning_delta(text);
                self.on_agent_reasoning_final()
            }
            EventMsg::AgentReasoningSectionBreak(_) => self.on_reasoning_section_break(),
            EventMsg::TaskStarted(_) => self.on_task_started(),
            EventMsg::TaskComplete(TaskCompleteEvent { last_agent_message }) => {
                self.on_task_complete(last_agent_message)
            }
            EventMsg::TokenCount(ev) => {
                self.set_token_info(ev.info);
                self.on_rate_limit_snapshot(ev.rate_limits);
            }
            EventMsg::Warning(WarningEvent { message }) => self.on_warning(message),
            EventMsg::Error(ErrorEvent { message, .. }) => self.on_error(message),
            EventMsg::McpStartupUpdate(ev) => self.on_mcp_startup_update(ev),
            EventMsg::McpStartupComplete(ev) => self.on_mcp_startup_complete(ev),
            EventMsg::TurnAborted(ev) => match ev.reason {
                TurnAbortReason::Interrupted => {
                    self.on_interrupted_turn(ev.reason);
                }
                TurnAbortReason::Replaced => {
                    self.on_error("Turn aborted: replaced by a new task".to_owned())
                }
                TurnAbortReason::ReviewEnded => {
                    self.on_interrupted_turn(ev.reason);
                }
            },
            EventMsg::PlanUpdate(update) => self.on_plan_update(update),
            EventMsg::ExecApprovalRequest(ev) => {
                // For replayed events, synthesize an empty id (these should not occur).
                self.on_exec_approval_request(id.unwrap_or_default(), ev)
            }
            EventMsg::ApplyPatchApprovalRequest(ev) => {
                self.on_apply_patch_approval_request(id.unwrap_or_default(), ev)
            }
            EventMsg::ElicitationRequest(ev) => {
                self.on_elicitation_request(ev);
            }
            EventMsg::ExecCommandBegin(ev) => self.on_exec_command_begin(ev),
            EventMsg::TerminalInteraction(ev) => self.on_terminal_interaction(ev),
            EventMsg::ExecCommandOutputDelta(delta) => self.on_exec_command_output_delta(delta),
            EventMsg::PatchApplyBegin(ev) => self.on_patch_apply_begin(ev),
            EventMsg::PatchApplyEnd(ev) => self.on_patch_apply_end(ev),
            EventMsg::ExecCommandEnd(ev) => self.on_exec_command_end(ev),
            EventMsg::ViewImageToolCall(ev) => self.on_view_image_tool_call(ev),
            EventMsg::McpToolCallBegin(ev) => self.on_mcp_tool_call_begin(ev),
            EventMsg::McpToolCallEnd(ev) => self.on_mcp_tool_call_end(ev),
            EventMsg::WebSearchBegin(ev) => self.on_web_search_begin(ev),
            EventMsg::WebSearchEnd(ev) => self.on_web_search_end(ev),
            EventMsg::GetHistoryEntryResponse(ev) => self.on_get_history_entry_response(ev),
            EventMsg::McpListToolsResponse(ev) => self.on_list_mcp_tools(ev),
            EventMsg::ListCustomPromptsResponse(ev) => self.on_list_custom_prompts(ev),
            EventMsg::ListSkillsResponse(ev) => self.on_list_skills(ev),
            EventMsg::SkillsUpdateAvailable => {
                self.submit_op(Op::ListSkills {
                    cwds: Vec::new(),
                    force_reload: true,
                });
            }
            EventMsg::ShutdownComplete => self.on_shutdown_complete(),
            EventMsg::TurnDiff(TurnDiffEvent { unified_diff }) => self.on_turn_diff(unified_diff),
            EventMsg::DeprecationNotice(ev) => self.on_deprecation_notice(ev),
            EventMsg::BackgroundEvent(BackgroundEventEvent { message }) => {
                self.on_background_event(message)
            }
            EventMsg::UndoStarted(ev) => self.on_undo_started(ev),
            EventMsg::UndoCompleted(ev) => self.on_undo_completed(ev),
            EventMsg::StreamError(StreamErrorEvent { message, .. }) => {
                self.on_stream_error(message)
            }
            EventMsg::RawResponseItem(ev) => {
                self.on_raw_response_item(ev);
            }
            EventMsg::UserMessage(ev) => {
                if from_replay {
                    self.on_user_message_event(ev);
                }
            }
            EventMsg::EnteredReviewMode(review_request) => {
                self.on_entered_review_mode(review_request)
            }
            EventMsg::ExitedReviewMode(review) => self.on_exited_review_mode(review),
            EventMsg::ContextCompacted(_) => self.on_agent_message("Context compacted".to_owned()),
            EventMsg::RalphLoopContinue(ev) => self.on_ralph_loop_continue(ev),
            EventMsg::RalphLoopStatus(ev) => self.on_ralph_loop_status(ev),
            EventMsg::RalphLoopComplete(ev) => self.on_ralph_loop_complete(ev),
            EventMsg::ItemStarted(_)
            | EventMsg::ItemCompleted(_)
            | EventMsg::AgentMessageContentDelta(_)
            | EventMsg::ReasoningContentDelta(_)
            | EventMsg::ReasoningRawContentDelta(_) => {}
        }
    }

    fn on_entered_review_mode(&mut self, review: ReviewRequest) {
        // Enter review mode and emit a concise banner
        if self.pre_review_token_info.is_none() {
            self.pre_review_token_info = Some(self.token_info.clone());
        }
        self.is_review_mode = true;
        let hint = review
            .user_facing_hint
            .unwrap_or_else(|| codex_core::review_prompts::user_facing_hint(&review.target));
        let banner = format!(">> Code review started: {hint} <<");
        self.add_to_history(history_cell::new_review_status_line(banner));
        self.request_redraw();
    }

    fn on_exited_review_mode(&mut self, review: ExitedReviewModeEvent) {
        // Leave review mode; if output is present, flush pending stream + show results.
        if let Some(output) = review.review_output {
            self.flush_answer_stream_with_separator();
            self.flush_interrupt_queue();
            self.flush_active_cell();

            if output.findings.is_empty() {
                let explanation = output.overall_explanation.trim().to_string();
                if explanation.is_empty() {
                    tracing::error!("Reviewer failed to output a response.");
                    self.add_to_history(history_cell::new_error_event(
                        "Reviewer failed to output a response.".to_owned(),
                    ));
                } else {
                    // Show explanation when there are no structured findings.
                    let mut rendered: Vec<ratatui::text::Line<'static>> = vec!["".into()];
                    append_markdown(&explanation, None, &mut rendered);
                    let body_cell = AgentMessageCell::new(rendered, false);
                    self.app_event_tx
                        .send(AppEvent::InsertHistoryCell(Box::new(body_cell)));
                }
            } else {
                let message_text =
                    codex_core::review_format::format_review_findings_block(&output.findings, None);
                let mut message_lines: Vec<ratatui::text::Line<'static>> = Vec::new();
                append_markdown(&message_text, None, &mut message_lines);
                let body_cell = AgentMessageCell::new(message_lines, true);
                self.app_event_tx
                    .send(AppEvent::InsertHistoryCell(Box::new(body_cell)));
            }
        }

        self.is_review_mode = false;
        self.restore_pre_review_token_info();
        // Append a finishing banner at the end of this turn.
        self.add_to_history(history_cell::new_review_status_line(
            "<< Code review finished >>".to_string(),
        ));
        self.request_redraw();
    }

    fn on_user_message_event(&mut self, event: UserMessageEvent) {
        let message = event.message.trim();
        if !message.is_empty() {
            self.add_to_history(history_cell::new_user_prompt(message.to_string()));
        }
    }

    fn request_exit(&self) {
        self.app_event_tx.send(AppEvent::ExitRequest);
    }

    fn on_raw_response_item(&mut self, event: RawResponseItemEvent) {
        let RawResponseItemEvent { item } = event;
        let ResponseItem::Message { role, content, .. } = item else {
            return;
        };

        if role != "assistant" {
            return;
        }

        let Some(conversation_id) = self.conversation_id else {
            return;
        };

        let mut saved_any = false;
        let mut last_saved_path: Option<PathBuf> = None;

        for content_item in content {
            if let ContentItem::InputImage { image_url } = content_item
                && let Some(path) = self.save_generated_image(&conversation_id, &image_url)
            {
                saved_any = true;
                last_saved_path = Some(path);
            }
        }

        if let (true, Some(path)) = (saved_any, last_saved_path) {
            let display = display_path_for(&path, &self.config.cwd);
            let hint = format!("{display} · run /open-image to open it");
            self.add_to_history(history_cell::new_info_event(
                "Generated image saved".to_string(),
                Some(hint),
            ));
            self.last_generated_image_path = Some(path);
            self.request_redraw();
        }
    }

    fn save_generated_image(
        &mut self,
        conversation_id: &ConversationId,
        image_url: &str,
    ) -> Option<PathBuf> {
        // Only handle data URLs of the form data:<mime>;base64,<data>.
        let without_prefix = image_url.strip_prefix("data:")?;
        let (meta, data_base64) = without_prefix.split_once(',')?;
        let (mime, encoding) = meta.split_once(';')?;
        if !encoding.eq_ignore_ascii_case("base64") {
            return None;
        }

        let bytes = match BASE64_STANDARD.decode(data_base64) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!("failed to decode generated image data: {err}");
                return None;
            }
        };

        let ext = match mime {
            "image/jpeg" | "image/jpg" => "jpg",
            "image/png" => "png",
            "image/gif" => "gif",
            "image/webp" => "webp",
            other => {
                tracing::warn!("saving generated image with unrecognized mime type `{other}`");
                "bin"
            }
        };

        // If batch processing is active, save to source directory with _processed suffix
        if let Some(batch_state) = &self.batch_image_state
            && let Some(current_image) = &batch_state.current_image
        {
            let source_dir = &batch_state.source_dir;
            let original_stem = current_image
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output");
            let processed_filename = format!("{original_stem}_processed.{ext}");
            let path = source_dir.join(processed_filename);
            if let Err(err) = std::fs::write(&path, &bytes) {
                tracing::warn!(
                    "failed to write batch processed image to {}: {err}",
                    path.display()
                );
                return None;
            }
            return Some(path);
        }

        // Default behavior: save to ~/.codex/images/{conversation_id}/
        let dir = self
            .config
            .codex_home
            .join("images")
            .join(conversation_id.to_string());
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!("failed to create images directory {}: {err}", dir.display());
            return None;
        }

        let index = self.next_generated_image_index;
        self.next_generated_image_index = self.next_generated_image_index.saturating_add(1);
        let filename = format!("{index:06}.{ext}");
        let path = dir.join(filename);
        if let Err(err) = std::fs::write(&path, &bytes) {
            tracing::warn!(
                "failed to write generated image to {}: {err}",
                path.display()
            );
            return None;
        }

        Some(path)
    }

    fn open_last_generated_image(&mut self) {
        let Some(path) = self.last_generated_image_path.clone() else {
            self.add_to_history(history_cell::new_info_event(
                "No generated image is available to open yet.".to_string(),
                None,
            ));
            self.request_redraw();
            return;
        };

        let display = display_path_for(&path, &self.config.cwd);

        match Self::open_image_in_viewer(&path) {
            Ok(()) => {
                self.add_to_history(history_cell::new_info_event(
                    "Opening generated image".to_string(),
                    Some(display),
                ));
            }
            Err(error) => {
                self.add_to_history(history_cell::new_error_event(format!(
                    "Failed to open generated image: {error}"
                )));
            }
        }

        self.request_redraw();
    }

    fn open_image_in_viewer(path: &Path) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        let cmd_result = std::process::Command::new("open").arg(path).spawn();

        #[cfg(target_os = "linux")]
        let cmd_result = std::process::Command::new("xdg-open").arg(path).spawn();

        #[cfg(target_os = "windows")]
        let cmd_result = std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn();

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let cmd_result = Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "opening images is not supported on this platform",
        ));

        cmd_result
            .map(|_| ())
            .map_err(|error| format!("failed to spawn image viewer: {error}"))
    }

    pub(crate) fn request_redraw(&mut self) {
        self.frame_requester.schedule_frame();
    }

    fn notify(&mut self, notification: Notification) {
        if !notification.allowed_for(&self.config.tui_notifications) {
            return;
        }
        self.maybe_play_completion_sound(&notification);
        self.pending_notification = Some(notification);
        self.request_redraw();
    }

    #[cfg(target_os = "macos")]
    fn maybe_play_completion_sound(&self, notification: &Notification) {
        if !matches!(notification, Notification::AgentTurnComplete { .. }) {
            return;
        }
        if let Err(error) = std::process::Command::new("afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .spawn()
        {
            debug!(error = %error, "failed to play completion sound");
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn maybe_play_completion_sound(&self, _notification: &Notification) {}

    pub(crate) fn maybe_post_pending_notification(&mut self, tui: &mut crate::tui::Tui) {
        if let Some(notif) = self.pending_notification.take() {
            tui.notify(notif.display());
        }
    }

    /// Mark the active cell as failed (✗) and flush it into history.
    fn finalize_active_cell_as_failed(&mut self) {
        if let Some(mut cell) = self.active_cell.take() {
            // Insert finalized cell into history and keep grouping consistent.
            if let Some(exec) = cell.as_any_mut().downcast_mut::<ExecCell>() {
                exec.mark_failed();
            } else if let Some(tool) = cell.as_any_mut().downcast_mut::<McpToolCallCell>() {
                tool.mark_failed();
            }
            self.add_boxed_history(cell);
        }
    }

    // If idle and there are queued inputs, submit exactly one to start the next turn.
    fn maybe_send_next_queued_input(&mut self) {
        if self.bottom_pane.is_task_running() {
            return;
        }
        if let Some(user_message) = self.queued_user_messages.pop_front() {
            if !(user_message.text.is_empty() && user_message.image_paths.is_empty()) {
                self.queued_turn_pending_start = true;
            }
            self.submit_user_message(user_message);
        }
        // Update the list to reflect the remaining queued messages (if any).
        self.refresh_queued_user_messages();
    }

    /// Rebuild and update the queued user messages from the current queue.
    fn refresh_queued_user_messages(&mut self) {
        let messages: Vec<String> = self
            .queued_user_messages
            .iter()
            .map(|m| m.text.clone())
            .collect();
        self.bottom_pane.set_queued_user_messages(messages);
    }

    pub(crate) fn add_diff_in_progress(&mut self) {
        self.request_redraw();
    }

    pub(crate) fn on_diff_complete(&mut self) {
        self.request_redraw();
    }

    pub(crate) fn add_status_output(&mut self) {
        let default_usage = TokenUsage::default();
        let (total_usage, context_usage) = if let Some(ti) = &self.token_info {
            (&ti.total_token_usage, Some(&ti.last_token_usage))
        } else {
            (&default_usage, Some(&default_usage))
        };
        let model_name = self.model_family.get_model_slug();
        self.add_to_history(crate::status::new_status_output(
            &self.config,
            self.auth_manager.as_ref(),
            &self.model_family,
            total_usage,
            context_usage,
            &self.conversation_id,
            self.rate_limit_snapshot.as_ref(),
            self.plan_type,
            Local::now(),
            model_name,
        ));
    }
    fn stop_rate_limit_poller(&mut self) {
        if let Some(handle) = self.rate_limit_poller.take() {
            handle.abort();
        }
    }

    fn prefetch_rate_limits(&mut self) {
        self.stop_rate_limit_poller();

        let Some(auth) = self.auth_manager.auth() else {
            return;
        };
        if auth.mode != AuthMode::ChatGPT {
            return;
        }

        let base_url = self.config.chatgpt_base_url.clone();
        let app_event_tx = self.app_event_tx.clone();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));

            loop {
                if let Some(snapshot) = fetch_rate_limits(base_url.clone(), auth.clone()).await {
                    app_event_tx.send(AppEvent::RateLimitSnapshotFetched(snapshot));
                }
                interval.tick().await;
            }
        });

        self.rate_limit_poller = Some(handle);
    }

    fn lower_cost_preset(&self) -> Option<ModelPreset> {
        let models = self.models_manager.try_list_models(&self.config).ok()?;
        models
            .iter()
            .find(|preset| preset.model == NUDGE_MODEL_SLUG)
            .cloned()
    }

    fn rate_limit_switch_prompt_hidden(&self) -> bool {
        self.config
            .notices
            .hide_rate_limit_model_nudge
            .unwrap_or(false)
    }

    fn maybe_show_pending_rate_limit_prompt(&mut self) {
        if self.rate_limit_switch_prompt_hidden() {
            self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Idle;
            return;
        }
        if !matches!(
            self.rate_limit_switch_prompt,
            RateLimitSwitchPromptState::Pending
        ) {
            return;
        }
        if let Some(preset) = self.lower_cost_preset() {
            self.open_rate_limit_switch_prompt(preset);
            self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Shown;
        } else {
            self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Idle;
        }
    }

    fn open_rate_limit_switch_prompt(&mut self, preset: ModelPreset) {
        let switch_model = preset.model.to_string();
        let display_name = preset.display_name.to_string();
        let default_effort: ReasoningEffortConfig = preset.default_reasoning_effort;

        let switch_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::CodexOp(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: Some(switch_model.clone()),
                effort: Some(Some(default_effort)),
                summary: None,
            }));
            tx.send(AppEvent::UpdateModel(switch_model.clone()));
            tx.send(AppEvent::UpdateReasoningEffort(Some(default_effort)));
        })];

        let keep_actions: Vec<SelectionAction> = Vec::new();
        let never_actions: Vec<SelectionAction> = vec![Box::new(|tx| {
            tx.send(AppEvent::UpdateRateLimitSwitchPromptHidden(true));
            tx.send(AppEvent::PersistRateLimitSwitchPromptHidden);
        })];
        let description = if preset.description.is_empty() {
            Some("Uses fewer credits for upcoming turns.".to_string())
        } else {
            Some(preset.description)
        };

        let items = vec![
            SelectionItem {
                name: format!("Switch to {display_name}"),
                description,
                selected_description: None,
                is_current: false,
                actions: switch_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Keep current model".to_string(),
                description: None,
                selected_description: None,
                is_current: false,
                actions: keep_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Keep current model (never show again)".to_string(),
                description: Some(
                    "Hide future rate limit reminders about switching models.".to_string(),
                ),
                selected_description: None,
                is_current: false,
                actions: never_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Approaching rate limits".to_string()),
            subtitle: Some(format!("Switch to {display_name} for lower credit usage?")),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    /// Open a popup to choose the model (stage 1). After selecting a model,
    /// a second popup is shown to choose the reasoning effort.
    pub(crate) fn open_model_popup(&mut self) {
        let current_model = self.config.model.clone().unwrap_or_default();
        let presets: Vec<ModelPreset> = match self.models_manager.try_list_models(&self.config) {
            Ok(models) => models,
            Err(_) => {
                self.add_info_message(
                    "Models are being updated; please try /model again in a moment.".to_string(),
                    None,
                );
                return;
            }
        };

        let current_label = presets
            .iter()
            .find(|preset| preset.model == current_model)
            .map(|preset| preset.display_name.to_string())
            .unwrap_or_else(|| current_model.clone());

        let (mut auto_presets, other_presets): (Vec<ModelPreset>, Vec<ModelPreset>) = presets
            .into_iter()
            .partition(|preset| Self::is_auto_model(&preset.model));

        if auto_presets.is_empty() {
            self.open_all_models_popup(other_presets);
            return;
        }

        auto_presets.sort_by_key(|preset| Self::auto_model_order(&preset.model));

        let mut items: Vec<SelectionItem> = auto_presets
            .into_iter()
            .map(|preset| {
                let description =
                    (!preset.description.is_empty()).then_some(preset.description.clone());
                let model = preset.model.clone();
                let actions = Self::model_selection_actions(
                    model.clone(),
                    Some(preset.default_reasoning_effort),
                );
                SelectionItem {
                    name: preset.display_name,
                    description,
                    is_current: model == current_model,
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        if !other_presets.is_empty() {
            let all_models = other_presets;
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenAllModelsPopup {
                    models: all_models.clone(),
                });
            })];

            let is_current = !items.iter().any(|item| item.is_current);
            let description = Some(format!(
                "Choose a specific model and reasoning level (current: {current_label})"
            ));

            items.push(SelectionItem {
                name: "All models".to_string(),
                description,
                is_current,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Select Model".to_string()),
            subtitle: Some("Pick a quick auto mode or browse all models.".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    fn is_auto_model(model: &str) -> bool {
        model.starts_with("codex-auto-")
    }

    fn auto_model_order(model: &str) -> usize {
        match model {
            "codex-auto-fast" => 0,
            "codex-auto-balanced" => 1,
            "codex-auto-thorough" => 2,
            _ => 3,
        }
    }

    pub(crate) fn open_all_models_popup(&mut self, presets: Vec<ModelPreset>) {
        if presets.is_empty() {
            self.add_info_message(
                "No additional models are available right now.".to_string(),
                None,
            );
            return;
        }

        let current_model = self.config.model.clone().unwrap_or_default();
        let mut items: Vec<SelectionItem> = Vec::new();
        for preset in presets.into_iter() {
            let description =
                (!preset.description.is_empty()).then_some(preset.description.to_string());
            let is_current = preset.model == current_model;
            let single_supported_effort = preset.supported_reasoning_efforts.len() == 1;
            let preset_for_action = preset.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                let preset_for_event = preset_for_action.clone();
                tx.send(AppEvent::OpenReasoningPopup {
                    model: preset_for_event,
                });
            })];
            items.push(SelectionItem {
                name: preset.display_name.clone(),
                description,
                is_current,
                actions,
                dismiss_on_select: single_supported_effort,
                ..Default::default()
            });
        }

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Select Model and Effort".to_string()),
            subtitle: Some(
                "Access legacy models by running codex -m <model_name> or in your config.toml"
                    .to_string(),
            ),
            footer_hint: Some("Press enter to select reasoning effort, or esc to dismiss.".into()),
            items,
            ..Default::default()
        });
    }

    fn model_selection_actions(
        model_for_action: String,
        effort_for_action: Option<ReasoningEffortConfig>,
    ) -> Vec<SelectionAction> {
        vec![Box::new(move |tx| {
            let effort_label = effort_for_action
                .map(|effort| effort.to_string())
                .unwrap_or_else(|| "default".to_string());
            tx.send(AppEvent::CodexOp(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: Some(model_for_action.clone()),
                effort: Some(effort_for_action),
                summary: None,
            }));
            tx.send(AppEvent::UpdateModel(model_for_action.clone()));
            tx.send(AppEvent::UpdateReasoningEffort(effort_for_action));
            tx.send(AppEvent::PersistModelSelection {
                model: model_for_action.clone(),
                effort: effort_for_action,
            });
            tracing::info!(
                "Selected model: {}, Selected effort: {}",
                model_for_action,
                effort_label
            );
        })]
    }

    /// Open a popup to choose the reasoning effort (stage 2) for the given model.
    pub(crate) fn open_reasoning_popup(&mut self, preset: ModelPreset) {
        let default_effort: ReasoningEffortConfig = preset.default_reasoning_effort;
        let supported = preset.supported_reasoning_efforts;

        let warn_effort = if supported
            .iter()
            .any(|option| option.effort == ReasoningEffortConfig::XHigh)
        {
            Some(ReasoningEffortConfig::XHigh)
        } else if supported
            .iter()
            .any(|option| option.effort == ReasoningEffortConfig::High)
        {
            Some(ReasoningEffortConfig::High)
        } else {
            None
        };
        let warning_text = warn_effort.map(|effort| {
            let effort_label = Self::reasoning_effort_label(effort);
            format!("⚠ {effort_label} reasoning effort can quickly consume Plus plan rate limits.")
        });
        let warn_for_model = preset.model.starts_with("gpt-5.1-codex")
            || preset.model.starts_with("gpt-5.1-codex-max");

        struct EffortChoice {
            stored: Option<ReasoningEffortConfig>,
            display: ReasoningEffortConfig,
        }
        let mut choices: Vec<EffortChoice> = Vec::new();
        for effort in ReasoningEffortConfig::iter() {
            if supported.iter().any(|option| option.effort == effort) {
                choices.push(EffortChoice {
                    stored: Some(effort),
                    display: effort,
                });
            }
        }
        if choices.is_empty() {
            choices.push(EffortChoice {
                stored: Some(default_effort),
                display: default_effort,
            });
        }

        if choices.len() == 1 {
            if let Some(effort) = choices.first().and_then(|c| c.stored) {
                self.apply_model_and_effort(preset.model, Some(effort));
            } else {
                self.apply_model_and_effort(preset.model, None);
            }
            return;
        }

        let default_choice: Option<ReasoningEffortConfig> = choices
            .iter()
            .any(|choice| choice.stored == Some(default_effort))
            .then_some(Some(default_effort))
            .flatten()
            .or_else(|| choices.iter().find_map(|choice| choice.stored))
            .or(Some(default_effort));

        let model_slug = preset.model.to_string();
        let is_current_model = self.config.model.as_deref() == Some(preset.model.as_str());
        let highlight_choice = if is_current_model {
            self.config.model_reasoning_effort
        } else {
            default_choice
        };
        let selection_choice = highlight_choice.or(default_choice);
        let initial_selected_idx = choices
            .iter()
            .position(|choice| choice.stored == selection_choice)
            .or_else(|| {
                selection_choice
                    .and_then(|effort| choices.iter().position(|choice| choice.display == effort))
            });
        let mut items: Vec<SelectionItem> = Vec::new();
        for choice in choices.iter() {
            let effort = choice.display;
            let mut effort_label = Self::reasoning_effort_label(effort).to_string();
            if choice.stored == default_choice {
                effort_label.push_str(" (default)");
            }

            let description = choice
                .stored
                .and_then(|effort| {
                    supported
                        .iter()
                        .find(|option| option.effort == effort)
                        .map(|option| option.description.to_string())
                })
                .filter(|text| !text.is_empty());

            let show_warning = warn_for_model && warn_effort == Some(effort);
            let selected_description = if show_warning {
                warning_text.as_ref().map(|warning_message| {
                    description.as_ref().map_or_else(
                        || warning_message.clone(),
                        |d| format!("{d}\n{warning_message}"),
                    )
                })
            } else {
                None
            };

            let model_for_action = model_slug.clone();
            let effort_for_action = choice.stored;
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::CodexOp(Op::OverrideTurnContext {
                    cwd: None,
                    approval_policy: None,
                    sandbox_policy: None,
                    model: Some(model_for_action.clone()),
                    effort: Some(effort_for_action),
                    summary: None,
                }));
                tx.send(AppEvent::UpdateModel(model_for_action.clone()));
                tx.send(AppEvent::UpdateReasoningEffort(effort_for_action));
                tx.send(AppEvent::PersistModelSelection {
                    model: model_for_action.clone(),
                    effort: effort_for_action,
                });
                tracing::info!(
                    "Selected model: {}, Selected effort: {}",
                    model_for_action,
                    effort_for_action
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "default".to_string())
                );
            })];

            items.push(SelectionItem {
                name: effort_label,
                description,
                selected_description,
                is_current: is_current_model && choice.stored == highlight_choice,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let mut header = ColumnRenderable::new();
        header.push(Line::from(
            format!("Select Reasoning Level for {model_slug}").bold(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            initial_selected_idx,
            ..Default::default()
        });
    }

    pub(crate) fn open_delegate_picker(
        &mut self,
        mut sessions: Vec<DelegatePickerSession>,
        detached_runs: Vec<DetachedRunSummary>,
        active_delegate: Option<&str>,
    ) {
        if sessions.is_empty() && detached_runs.is_empty() {
            self.add_info_message(
                "No delegate sessions available.".to_string(),
                Some("Ask the main agent to delegate a task first.".to_string()),
            );
            return;
        }

        sessions.sort_by(|a, b| {
            b.summary
                .last_interacted_at
                .cmp(&a.summary.last_interacted_at)
        });

        let mut items: Vec<SelectionItem> = Vec::new();

        if active_delegate.is_some() {
            let actions: Vec<SelectionAction> =
                vec![Box::new(|tx| tx.send(AppEvent::ExitDelegateSession))];
            items.push(SelectionItem {
                name: "Return to main agent".to_string(),
                description: None,
                is_current: false,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        for entry in sessions {
            let summary = entry.summary;
            let run_id = entry.run_id;
            let conversation_id = summary.conversation_id.clone();
            let prefix = if summary.mode == DelegateSessionMode::Detached {
                "Detached · "
            } else {
                ""
            };
            let label = format!(
                "{prefix}#{} · {}",
                summary.agent_id.as_str(),
                Self::format_delegate_timestamp(summary.last_interacted_at)
            );
            let description = Some(summary.cwd.display().to_string());
            let is_current = active_delegate == Some(conversation_id.as_str());
            let conversation_id_for_action = conversation_id.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::EnterDelegateSession(
                    conversation_id_for_action.clone(),
                ));
            })];
            items.push(SelectionItem {
                name: label,
                description,
                is_current,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });

            if summary.mode == DelegateSessionMode::Detached
                && let Some(run_id) = run_id.clone()
            {
                let dismiss_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                    tx.send(AppEvent::DismissDetachedRun(run_id.clone()));
                })];
                items.push(SelectionItem {
                    name: format!("  Dismiss detached run for #{}", summary.agent_id.as_str()),
                    description: Some("Remove this detached run from the list.".to_string()),
                    is_current: false,
                    actions: dismiss_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                });
            }
        }

        for detached in detached_runs {
            let run_id = detached.run_id.clone();
            let status = detached.status.clone();
            let label = match &status {
                DetachedRunStatusSummary::Pending => format!(
                    "Pending · #{} (started {})",
                    detached.agent_id.as_str(),
                    Self::format_delegate_timestamp(detached.started_at)
                ),
                DetachedRunStatusSummary::Failed { .. } => {
                    format!("Failed · #{}", detached.agent_id.as_str())
                }
            };
            let description = match &status {
                DetachedRunStatusSummary::Pending => {
                    let mut text = String::from(
                        "Run is still executing; you'll be able to dismiss it once it finishes.",
                    );
                    if let Some(preview) = detached.prompt_preview.as_ref() {
                        text.push_str("\nPrompt: ");
                        text.push_str(preview);
                    }
                    Some(text)
                }
                DetachedRunStatusSummary::Failed { error, .. } => Some(format!("Error: {error}")),
            };
            let (actions, dismiss_on_select): (Vec<SelectionAction>, bool) = match status {
                DetachedRunStatusSummary::Pending => (Vec::new(), false),
                DetachedRunStatusSummary::Failed { .. } => {
                    let run_id_clone = run_id.clone();
                    (
                        vec![Box::new(move |tx: &AppEventSender| {
                            tx.send(AppEvent::DismissDetachedRun(run_id_clone.clone()));
                        }) as SelectionAction],
                        true,
                    )
                }
            };
            items.push(SelectionItem {
                name: label,
                description,
                is_current: false,
                actions,
                dismiss_on_select,
                ..Default::default()
            });
        }

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Switch agent".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    fn format_delegate_timestamp(time: SystemTime) -> String {
        let utc: DateTime<Utc> = time.into();
        utc.with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    fn reasoning_effort_label(effort: ReasoningEffortConfig) -> &'static str {
        match effort {
            ReasoningEffortConfig::None => "None",
            ReasoningEffortConfig::Minimal => "Minimal",
            ReasoningEffortConfig::Low => "Low",
            ReasoningEffortConfig::Medium => "Medium",
            ReasoningEffortConfig::High => "High",
            ReasoningEffortConfig::XHigh => "Extra high",
        }
    }

    fn apply_model_and_effort(&self, model: String, effort: Option<ReasoningEffortConfig>) {
        self.app_event_tx
            .send(AppEvent::CodexOp(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: None,
                sandbox_policy: None,
                model: Some(model.clone()),
                effort: Some(effort),
                summary: None,
            }));
        self.app_event_tx.send(AppEvent::UpdateModel(model.clone()));
        self.app_event_tx
            .send(AppEvent::UpdateReasoningEffort(effort));
        self.app_event_tx.send(AppEvent::PersistModelSelection {
            model: model.clone(),
            effort,
        });
        tracing::info!(
            "Selected model: {}, Selected effort: {}",
            model,
            effort
                .map(|e| e.to_string())
                .unwrap_or_else(|| "default".to_string())
        );
    }

    /// Open a popup to choose the approvals mode (ask for approval policy + sandbox policy).
    pub(crate) fn open_approvals_popup(&mut self) {
        let current_approval = self.config.approval_policy.value();
        let current_sandbox = self.config.sandbox_policy.clone();
        let mut items: Vec<SelectionItem> = Vec::new();
        let presets: Vec<ApprovalPreset> = builtin_approval_presets();
        for preset in presets.into_iter() {
            let is_current =
                Self::preset_matches_current(current_approval, &current_sandbox, &preset);
            let name = preset.label.to_string();
            let description_text = preset.description;
            let description = Some(description_text.to_string());
            let requires_confirmation = preset.id == "full-access"
                && !self
                    .config
                    .notices
                    .hide_full_access_warning
                    .unwrap_or(false);
            let actions: Vec<SelectionAction> = if requires_confirmation {
                let preset_clone = preset.clone();
                vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenFullAccessConfirmation {
                        preset: preset_clone.clone(),
                    });
                })]
            } else if preset.id == "auto" {
                #[cfg(target_os = "windows")]
                {
                    if codex_core::get_platform_sandbox().is_none() {
                        let preset_clone = preset.clone();
                        vec![Box::new(move |tx| {
                            tx.send(AppEvent::OpenWindowsSandboxEnablePrompt {
                                preset: preset_clone.clone(),
                            });
                        })]
                    } else if let Some((sample_paths, extra_count, failed_scan)) =
                        self.world_writable_warning_details()
                    {
                        let preset_clone = preset.clone();
                        vec![Box::new(move |tx| {
                            tx.send(AppEvent::OpenWorldWritableWarningConfirmation {
                                preset: Some(preset_clone.clone()),
                                sample_paths: sample_paths.clone(),
                                extra_count,
                                failed_scan,
                            });
                        })]
                    } else {
                        Self::approval_preset_actions(preset.approval, preset.sandbox.clone())
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    Self::approval_preset_actions(preset.approval, preset.sandbox.clone())
                }
            } else {
                Self::approval_preset_actions(preset.approval, preset.sandbox.clone())
            };
            items.push(SelectionItem {
                name,
                description,
                is_current,
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Select Approval Mode".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(()),
            ..Default::default()
        });
    }

    fn approval_preset_actions(
        approval: AskForApproval,
        sandbox: SandboxPolicy,
    ) -> Vec<SelectionAction> {
        vec![Box::new(move |tx| {
            let sandbox_clone = sandbox.clone();
            tx.send(AppEvent::CodexOp(Op::OverrideTurnContext {
                cwd: None,
                approval_policy: Some(approval),
                sandbox_policy: Some(sandbox_clone.clone()),
                model: None,
                effort: None,
                summary: None,
            }));
            tx.send(AppEvent::UpdateAskForApprovalPolicy(approval));
            tx.send(AppEvent::UpdateSandboxPolicy(sandbox_clone));
        })]
    }

    fn preset_matches_current(
        current_approval: AskForApproval,
        current_sandbox: &SandboxPolicy,
        preset: &ApprovalPreset,
    ) -> bool {
        if current_approval != preset.approval {
            return false;
        }
        matches!(
            (&preset.sandbox, current_sandbox),
            (SandboxPolicy::ReadOnly, SandboxPolicy::ReadOnly)
                | (
                    SandboxPolicy::DangerFullAccess,
                    SandboxPolicy::DangerFullAccess
                )
                | (
                    SandboxPolicy::WorkspaceWrite { .. },
                    SandboxPolicy::WorkspaceWrite { .. }
                )
        )
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn world_writable_warning_details(&self) -> Option<(Vec<String>, usize, bool)> {
        if self
            .config
            .notices
            .hide_world_writable_warning
            .unwrap_or(false)
        {
            return None;
        }
        let cwd = self.config.cwd.clone();
        let env_map: std::collections::HashMap<String, String> = std::env::vars().collect();
        match codex_windows_sandbox::apply_world_writable_scan_and_denies(
            self.config.codex_home.as_path(),
            cwd.as_path(),
            &env_map,
            &self.config.sandbox_policy,
            Some(self.config.codex_home.as_path()),
        ) {
            Ok(_) => None,
            Err(_) => Some((Vec::new(), 0, true)),
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[allow(dead_code)]
    pub(crate) fn world_writable_warning_details(&self) -> Option<(Vec<String>, usize, bool)> {
        None
    }

    pub(crate) fn open_full_access_confirmation(&mut self, preset: ApprovalPreset) {
        let approval = preset.approval;
        let sandbox = preset.sandbox;
        let mut header_children: Vec<Box<dyn Renderable>> = Vec::new();
        let title_line = Line::from("Enable full access?").bold();
        let info_line = Line::from(vec![
            "When Codex runs with full access, it can edit any file on your computer and run commands with network, without your approval. "
                .into(),
            "Exercise caution when enabling full access. This significantly increases the risk of data loss, leaks, or unexpected behavior."
                .fg(Color::Red),
        ]);
        header_children.push(Box::new(title_line));
        header_children.push(Box::new(
            Paragraph::new(vec![info_line]).wrap(Wrap { trim: false }),
        ));
        let header = ColumnRenderable::with(header_children);

        let mut accept_actions = Self::approval_preset_actions(approval, sandbox.clone());
        accept_actions.push(Box::new(|tx| {
            tx.send(AppEvent::UpdateFullAccessWarningAcknowledged(true));
        }));

        let mut accept_and_remember_actions = Self::approval_preset_actions(approval, sandbox);
        accept_and_remember_actions.push(Box::new(|tx| {
            tx.send(AppEvent::UpdateFullAccessWarningAcknowledged(true));
            tx.send(AppEvent::PersistFullAccessWarningAcknowledged);
        }));

        let deny_actions: Vec<SelectionAction> = vec![Box::new(|tx| {
            tx.send(AppEvent::OpenApprovalsPopup);
        })];

        let items = vec![
            SelectionItem {
                name: "Yes, continue anyway".to_string(),
                description: Some("Apply full access for this session".to_string()),
                actions: accept_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Yes, and don't ask again".to_string(),
                description: Some("Enable full access and remember this choice".to_string()),
                actions: accept_and_remember_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Cancel".to_string(),
                description: Some("Go back without enabling full access".to_string()),
                actions: deny_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(header),
            ..Default::default()
        });
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn open_world_writable_warning_confirmation(
        &mut self,
        preset: Option<ApprovalPreset>,
        sample_paths: Vec<String>,
        extra_count: usize,
        failed_scan: bool,
    ) {
        let (approval, sandbox) = match &preset {
            Some(p) => (Some(p.approval), Some(p.sandbox.clone())),
            None => (None, None),
        };
        let mut header_children: Vec<Box<dyn Renderable>> = Vec::new();
        let describe_policy = |policy: &SandboxPolicy| match policy {
            SandboxPolicy::WorkspaceWrite { .. } => "Agent mode",
            SandboxPolicy::ReadOnly => "Read-Only mode",
            _ => "Agent mode",
        };
        let mode_label = preset
            .as_ref()
            .map(|p| describe_policy(&p.sandbox))
            .unwrap_or_else(|| describe_policy(&self.config.sandbox_policy));
        let info_line = if failed_scan {
            Line::from(vec![
                "We couldn't complete the world-writable scan, so protections cannot be verified. "
                    .into(),
                format!("The Windows sandbox cannot guarantee protection in {mode_label}.")
                    .fg(Color::Red),
            ])
        } else {
            Line::from(vec![
                "The Windows sandbox cannot protect writes to folders that are writable by Everyone.".into(),
                " Consider removing write access for Everyone from the following folders:".into(),
            ])
        };
        header_children.push(Box::new(
            Paragraph::new(vec![info_line]).wrap(Wrap { trim: false }),
        ));

        if !sample_paths.is_empty() {
            // Show up to three examples and optionally an "and X more" line.
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(""));
            for p in &sample_paths {
                lines.push(Line::from(format!("  - {p}")));
            }
            if extra_count > 0 {
                lines.push(Line::from(format!("and {extra_count} more")));
            }
            header_children.push(Box::new(Paragraph::new(lines).wrap(Wrap { trim: false })));
        }
        let header = ColumnRenderable::with(header_children);

        // Build actions ensuring acknowledgement happens before applying the new sandbox policy,
        // so downstream policy-change hooks don't re-trigger the warning.
        let mut accept_actions: Vec<SelectionAction> = Vec::new();
        // Suppress the immediate re-scan only when a preset will be applied (i.e., via /approvals),
        // to avoid duplicate warnings from the ensuing policy change.
        if preset.is_some() {
            accept_actions.push(Box::new(|tx| {
                tx.send(AppEvent::SkipNextWorldWritableScan);
            }));
        }
        if let (Some(approval), Some(sandbox)) = (approval, sandbox.clone()) {
            accept_actions.extend(Self::approval_preset_actions(approval, sandbox));
        }

        let mut accept_and_remember_actions: Vec<SelectionAction> = Vec::new();
        accept_and_remember_actions.push(Box::new(|tx| {
            tx.send(AppEvent::UpdateWorldWritableWarningAcknowledged(true));
            tx.send(AppEvent::PersistWorldWritableWarningAcknowledged);
        }));
        if let (Some(approval), Some(sandbox)) = (approval, sandbox) {
            accept_and_remember_actions.extend(Self::approval_preset_actions(approval, sandbox));
        }

        let items = vec![
            SelectionItem {
                name: "Continue".to_string(),
                description: Some(format!("Apply {mode_label} for this session")),
                actions: accept_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Continue and don't warn again".to_string(),
                description: Some(format!("Enable {mode_label} and remember this choice")),
                actions: accept_and_remember_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(header),
            ..Default::default()
        });
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn open_world_writable_warning_confirmation(
        &mut self,
        _preset: Option<ApprovalPreset>,
        _sample_paths: Vec<String>,
        _extra_count: usize,
        _failed_scan: bool,
    ) {
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn open_windows_sandbox_enable_prompt(&mut self, preset: ApprovalPreset) {
        use ratatui_macros::line;

        let mut header = ColumnRenderable::new();
        header.push(*Box::new(
            Paragraph::new(vec![
                line!["Agent mode on Windows uses an experimental sandbox to limit network and filesystem access.".bold()],
                line![
                    "Learn more: https://developers.openai.com/codex/windows"
                ],
            ])
            .wrap(Wrap { trim: false }),
        ));

        let preset_clone = preset;
        let items = vec![
            SelectionItem {
                name: "Enable experimental sandbox".to_string(),
                description: None,
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::EnableWindowsSandboxForAgentMode {
                        preset: preset_clone.clone(),
                    });
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Go back".to_string(),
                description: None,
                actions: vec![Box::new(|tx| {
                    tx.send(AppEvent::OpenApprovalsPopup);
                })],
                dismiss_on_select: true,
                ..Default::default()
            },
        ];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: None,
            footer_hint: Some(standard_popup_hint_line()),
            items,
            header: Box::new(header),
            ..Default::default()
        });
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn open_windows_sandbox_enable_prompt(&mut self, _preset: ApprovalPreset) {}

    #[cfg(target_os = "windows")]
    pub(crate) fn maybe_prompt_windows_sandbox_enable(&mut self) {
        if self.config.forced_auto_mode_downgraded_on_windows
            && codex_core::get_platform_sandbox().is_none()
            && let Some(preset) = builtin_approval_presets()
                .into_iter()
                .find(|preset| preset.id == "auto")
        {
            self.open_windows_sandbox_enable_prompt(preset);
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(crate) fn maybe_prompt_windows_sandbox_enable(&mut self) {}

    #[cfg(target_os = "windows")]
    pub(crate) fn clear_forced_auto_mode_downgrade(&mut self) {
        self.config.forced_auto_mode_downgraded_on_windows = false;
    }

    #[cfg(not(target_os = "windows"))]
    #[allow(dead_code)]
    pub(crate) fn clear_forced_auto_mode_downgrade(&mut self) {}

    /// Set the approval policy in the widget's config copy.
    pub(crate) fn set_approval_policy(&mut self, policy: AskForApproval) {
        if let Err(err) = self.config.approval_policy.set(policy) {
            tracing::warn!(%err, "failed to set approval_policy on chat config");
        }
    }

    /// Set the sandbox policy in the widget's config copy.
    pub(crate) fn set_sandbox_policy(&mut self, policy: SandboxPolicy) {
        #[cfg(target_os = "windows")]
        let should_clear_downgrade = !matches!(policy, SandboxPolicy::ReadOnly)
            || codex_core::get_platform_sandbox().is_some();

        self.config.sandbox_policy = policy;

        #[cfg(target_os = "windows")]
        if should_clear_downgrade {
            self.config.forced_auto_mode_downgraded_on_windows = false;
        }
    }

    pub(crate) fn set_full_access_warning_acknowledged(&mut self, acknowledged: bool) {
        self.config.notices.hide_full_access_warning = Some(acknowledged);
    }

    pub(crate) fn set_world_writable_warning_acknowledged(&mut self, acknowledged: bool) {
        self.config.notices.hide_world_writable_warning = Some(acknowledged);
    }

    pub(crate) fn set_rate_limit_switch_prompt_hidden(&mut self, hidden: bool) {
        self.config.notices.hide_rate_limit_model_nudge = Some(hidden);
        if hidden {
            self.rate_limit_switch_prompt = RateLimitSwitchPromptState::Idle;
        }
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn world_writable_warning_hidden(&self) -> bool {
        self.config
            .notices
            .hide_world_writable_warning
            .unwrap_or(false)
    }

    /// Set the reasoning effort in the widget's config copy.
    pub(crate) fn set_reasoning_effort(&mut self, effort: Option<ReasoningEffortConfig>) {
        self.config.model_reasoning_effort = effort;
    }

    /// Set the model in the widget's config copy.
    pub(crate) fn set_model(&mut self, model: &str, model_family: ModelFamily) {
        self.session_header.set_model(model);
        self.model_family = model_family;
        self.config.model = Some(model.to_string());
    }

    pub(crate) fn add_info_message(&mut self, message: String, hint: Option<String>) {
        self.add_to_history(history_cell::new_info_event(message, hint));
        self.request_redraw();
    }

    pub(crate) fn add_plain_history_lines(&mut self, lines: Vec<Line<'static>>) {
        self.add_boxed_history(Box::new(PlainHistoryCell::new(lines)));
        self.request_redraw();
    }

    pub(crate) fn add_error_message(&mut self, message: String) {
        self.add_to_history(history_cell::new_error_event(message));
        self.request_redraw();
    }

    pub(crate) fn add_mcp_output(&mut self) {
        if self.config.mcp_servers.is_empty() {
            self.add_to_history(history_cell::empty_mcp_output());
        } else {
            self.submit_op(Op::ListMcpTools);
        }
    }

    pub(crate) fn handle_tumix_command(&mut self, user_prompt: Option<String>) {
        // If no prompt provided, show help instead of starting TUMIX
        if user_prompt.is_none() {
            let help_msg = "🚀 **TUMIX** - 多智能体并行执行框架\n\n\
                 **用法：** `/tumix <任务描述>`\n\n\
                 **示例：**\n\
                 • `/tumix 实现一个Rust自动微分库`\n\
                 • `/tumix 优化这段代码的性能`\n\
                 • `/tumix 设计分布式缓存系统`\n\n\
                 **工作流程：**\n\
                 1. Meta-agent 分析任务复杂度，灵活设计专家团队（2-15个agent）\n\
                 2. 每个 agent 在独立的 Git worktree 中工作\n\
                 3. 所有 agents 并行执行\n\
                 4. 结果保存到 `.tumix/round1_sessions.json`\n\
                 5. 创建分支：`round1-agent-01`, `round1-agent-02`...\n\n\
                 💡 **Agent数量根据任务自动调整：**\n\
                 • 简单任务 → 2-3个agent\n\
                 • 中等任务 → 4-6个agent\n\
                 • 复杂任务 → 7-10个agent\n\
                 • 超大任务 → 10-15个agent\n\n\
                 _请提供任务描述以启动 TUMIX_";

            self.add_to_history(history_cell::new_info_event(help_msg.to_string(), None));
            self.request_redraw();
            return;
        }

        let session_id = match &self.conversation_id {
            Some(id) => id.to_string(),
            None => {
                self.add_to_history(history_cell::new_error_event(
                    "Cannot run `/tumix`: No active session".to_string(),
                ));
                self.request_redraw();
                return;
            }
        };

        let prompt_text = user_prompt.as_deref().unwrap_or("").trim();
        let session_short = session_id.chars().take(8).collect::<String>();
        let run_id = format!("tumix-{}", Uuid::new_v4());
        let display_prompt = if prompt_text.is_empty() {
            format!("会话：{session_short}")
        } else {
            format!("会话：{session_short} · 任务：{prompt_text}")
        };

        self.app_event_tx.send(AppEvent::TumixRunRequested {
            run_id,
            session_id,
            user_prompt,
            display_prompt,
        });
    }

    pub(crate) fn handle_tumix_stop_command(&mut self, target: Option<String>) {
        let target_session = target.as_ref().and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        if let Some(session_id) = target_session {
            match codex_tumix::cancel_run(&session_id) {
                Some(run) => {
                    let short = run.run_id.chars().take(8).collect::<String>();
                    let msg = format!(
                        "🛑 Requested cancellation for TUMIX run {short}\n\
                         • Session: {session}\n\
                         • Run ID: {run_id}",
                        short = short,
                        session = session_id,
                        run_id = run.run_id
                    );
                    self.add_to_history(history_cell::new_info_event(msg, None));
                }
                None => {
                    let msg = format!("⚠️ No active TUMIX run found for session `{session_id}`.");
                    self.add_to_history(history_cell::new_error_event(msg));
                }
            }
            self.request_redraw();
            return;
        }

        let cancelled = codex_tumix::cancel_all_runs();
        if cancelled.is_empty() {
            self.add_to_history(history_cell::new_info_event(
                "ℹ️ There are no active TUMIX runs to stop.".to_string(),
                None,
            ));
        } else {
            let lines = cancelled
                .iter()
                .map(|run| {
                    let short = run.run_id.chars().take(8).collect::<String>();
                    format!("  • Session: {} (run {})", run.session_id, short)
                })
                .collect::<Vec<_>>()
                .join("\n");
            let msg = format!(
                "🛑 Requested cancellation for {} active TUMIX run(s):\n{}",
                cancelled.len(),
                lines
            );
            self.add_to_history(history_cell::new_info_event(msg, None));
        }
        self.request_redraw();
    }

    pub(crate) fn handle_ralph_loop_command(&mut self, args: Option<String>) {
        let Some(raw_args) = args else {
            self.add_to_history(history_cell::new_info_event(
                ralph_loop_help_text().to_string(),
                None,
            ));
            self.request_redraw();
            return;
        };

        let raw_args = raw_args.trim();
        if raw_args.is_empty() {
            self.add_to_history(history_cell::new_info_event(
                ralph_loop_help_text().to_string(),
                None,
            ));
            self.request_redraw();
            return;
        }

        let parsed = match codex_protocol::slash_commands::SlashCommand::parse(&format!(
            "/ralph-loop {raw_args}",
        )) {
            Ok(cmd) => cmd,
            Err(err) => {
                self.add_to_history(history_cell::new_error_event(format!(
                    "Invalid /ralph-loop command: {err}",
                )));
                self.request_redraw();
                return;
            }
        };

        let codex_protocol::slash_commands::SlashCommand::RalphLoop(cmd) = parsed else {
            self.add_to_history(history_cell::new_error_event(
                "Unexpected command parsed for /ralph-loop.".to_string(),
            ));
            self.request_redraw();
            return;
        };

        let prompt = cmd
            .prompt
            .clone()
            .or_else(|| self.last_user_message.clone());
        let Some(prompt) = prompt.filter(|p| !p.trim().is_empty()) else {
            self.add_to_history(history_cell::new_error_event(
                "Please provide a prompt (positional or with --prompt), or send a message first.\n\
                 Example: /ralph-loop \"Build API. Output <promise>COMPLETE</promise> when done.\" -n 30"
                    .to_string(),
            ));
            self.request_redraw();
            return;
        };

        let state = RalphLoopState::new_with_delay(
            prompt.clone(),
            cmd.max_iterations,
            cmd.completion_promise,
            cmd.delay_seconds,
        );
        self.ralph_loop_state = Some(state.clone());

        if let Err(err) = save_ralph_state_file(&self.config.cwd, &state) {
            tracing::warn!("failed to save ralph state file: {err}");
        }

        let max_iterations_label = if state.max_iterations == 0 {
            "unlimited".to_string()
        } else {
            state.max_iterations.to_string()
        };
        let delay_label = if state.delay_seconds > 0 {
            format!("{}s", state.delay_seconds)
        } else {
            "none".to_string()
        };
        let truncated = truncate_string(&prompt, 100);
        let status = format!(
            "🔄 Ralph Loop activated!\n\
             \n\
             Iteration: 1\n\
             Max iterations: {max_iterations_label}\n\
             Completion promise: <promise>{completion_promise}</promise>\n\
             Delay between iterations: {delay_label}\n\
             \n\
             To stop: output <promise>{completion_promise}</promise> (ONLY when TRUE)\n\
             To cancel: /cancel-ralph\n\
             \n\
             State file: {state_file}\n\
             \n\
             🔄 {truncated}",
            completion_promise = state.completion_promise,
            state_file = ralph_state_file_path(&self.config.cwd).display(),
        );
        self.add_to_history(history_cell::new_info_event(status, None));
        self.request_redraw();

        self.submit_user_message(prompt.into());
    }

    pub(crate) fn handle_cancel_ralph_command(&mut self) {
        let Some(state) = self.ralph_loop_state.take() else {
            self.add_to_history(history_cell::new_info_event(
                "ℹ️ There is no active Ralph loop to cancel.".to_string(),
                None,
            ));
            self.request_redraw();
            return;
        };

        if let Err(err) = cleanup_ralph_state_file(&self.config.cwd) {
            tracing::warn!("failed to cleanup ralph state file: {err}");
        }

        let duration_seconds = calculate_duration_seconds(&state.started_at);
        let msg = format!(
            "🛑 Ralph Loop cancelled ({iterations} iteration(s), {duration_seconds:.2}s).",
            iterations = state.iteration,
        );
        self.add_to_history(history_cell::new_info_event(msg, None));
        self.request_redraw();
    }

    /// Handle the delayed continuation of Ralph Loop after the configured delay.
    pub(crate) fn handle_ralph_loop_delayed_continue(&mut self) {
        let Some(state) = self.ralph_loop_state.as_ref() else {
            // Ralph loop was cancelled during the delay
            return;
        };

        let prompt = state.original_prompt.clone();

        // Re-inject the SAME original prompt (Ralph technique).
        self.queued_user_messages.push_front(prompt.into());
        self.refresh_queued_user_messages();
        self.maybe_send_next_queued_input();
        self.request_redraw();
    }

    /// Forward file-search results to the bottom pane.
    pub(crate) fn apply_file_search_result(&mut self, query: String, matches: Vec<FileMatch>) {
        self.bottom_pane.on_file_search_result(query, matches);
    }

    /// Handle Ctrl-C key press.
    fn on_ctrl_c(&mut self) {
        if self.bottom_pane.on_ctrl_c() == CancellationEvent::Handled {
            return;
        }

        if self.bottom_pane.is_task_running() {
            self.bottom_pane.show_ctrl_c_quit_hint();
            self.submit_op(Op::Interrupt);
            return;
        }

        self.submit_op(Op::Shutdown);
    }

    pub(crate) fn composer_is_empty(&self) -> bool {
        self.bottom_pane.composer_is_empty()
    }

    /// True when the UI is in the regular composer state with no running task,
    /// no modal overlay (e.g. approvals or status indicator), and no composer popups.
    /// In this state Esc-Esc backtracking is enabled.
    pub(crate) fn is_normal_backtrack_mode(&self) -> bool {
        self.bottom_pane.is_normal_backtrack_mode()
    }

    pub(crate) fn insert_str(&mut self, text: &str) {
        self.bottom_pane.insert_str(text);
    }

    /// Replace the composer content with the provided text and reset cursor.
    pub(crate) fn set_composer_text(&mut self, text: String) {
        self.bottom_pane.set_composer_text(text);
    }

    pub(crate) fn show_esc_backtrack_hint(&mut self) {
        self.bottom_pane.show_esc_backtrack_hint();
    }

    pub(crate) fn clear_esc_backtrack_hint(&mut self) {
        self.bottom_pane.clear_esc_backtrack_hint();
    }
    /// Forward an `Op` directly to codex.
    pub(crate) fn submit_op(&self, op: Op) {
        // Record outbound operation for session replay fidelity.
        crate::session_log::log_outbound_op(&op);
        if let Err(e) = self.codex_op_tx.send(op) {
            tracing::error!("failed to submit op: {e}");
        }
    }

    fn on_list_mcp_tools(&mut self, ev: McpListToolsResponseEvent) {
        self.add_to_history(history_cell::new_mcp_tools_output(
            &self.config,
            ev.tools,
            ev.resources,
            ev.resource_templates,
            &ev.auth_statuses,
        ));
    }

    fn on_list_custom_prompts(&mut self, ev: ListCustomPromptsResponseEvent) {
        let len = ev.custom_prompts.len();
        debug!("received {len} custom prompts");
        // Forward to bottom pane so the slash popup can show them now.
        self.bottom_pane.set_custom_prompts(ev.custom_prompts);
    }

    fn on_list_skills(&mut self, ev: ListSkillsResponseEvent) {
        let len = ev.skills.len();
        debug!("received {len} skills");
    }

    pub(crate) fn open_review_popup(&mut self) {
        let mut items: Vec<SelectionItem> = Vec::new();

        items.push(SelectionItem {
            name: "Review against a base branch".to_string(),
            description: Some("(PR Style)".into()),
            actions: vec![Box::new({
                let cwd = self.config.cwd.clone();
                move |tx| {
                    tx.send(AppEvent::OpenReviewBranchPicker(cwd.clone()));
                }
            })],
            dismiss_on_select: false,
            ..Default::default()
        });

        items.push(SelectionItem {
            name: "Review uncommitted changes".to_string(),
            actions: vec![Box::new(move |tx: &AppEventSender| {
                tx.send(AppEvent::CodexOp(Op::Review {
                    review_request: ReviewRequest {
                        target: ReviewTarget::UncommittedChanges,
                        user_facing_hint: None,
                    },
                }));
            })],
            dismiss_on_select: true,
            ..Default::default()
        });

        // New: Review a specific commit (opens commit picker)
        items.push(SelectionItem {
            name: "Review a commit".to_string(),
            actions: vec![Box::new({
                let cwd = self.config.cwd.clone();
                move |tx| {
                    tx.send(AppEvent::OpenReviewCommitPicker(cwd.clone()));
                }
            })],
            dismiss_on_select: false,
            ..Default::default()
        });

        items.push(SelectionItem {
            name: "Custom review instructions".to_string(),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenReviewCustomPrompt);
            })],
            dismiss_on_select: false,
            ..Default::default()
        });

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Select a review preset".into()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) async fn show_review_branch_picker(&mut self, cwd: &Path) {
        let branches = local_git_branches(cwd).await;
        let current_branch = current_branch_name(cwd)
            .await
            .unwrap_or_else(|| "(detached HEAD)".to_string());
        let mut items: Vec<SelectionItem> = Vec::with_capacity(branches.len());

        for option in branches {
            let branch = option.clone();
            items.push(SelectionItem {
                name: format!("{current_branch} -> {branch}"),
                actions: vec![Box::new(move |tx3: &AppEventSender| {
                    tx3.send(AppEvent::CodexOp(Op::Review {
                        review_request: ReviewRequest {
                            target: ReviewTarget::BaseBranch {
                                branch: branch.clone(),
                            },
                            user_facing_hint: None,
                        },
                    }));
                })],
                dismiss_on_select: true,
                search_value: Some(option),
                ..Default::default()
            });
        }

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Select a base branch".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Type to search branches".to_string()),
            ..Default::default()
        });
    }

    pub(crate) async fn show_review_commit_picker(&mut self, cwd: &Path) {
        let commits = codex_core::git_info::recent_commits(cwd, 100).await;

        let mut items: Vec<SelectionItem> = Vec::with_capacity(commits.len());
        for entry in commits {
            let subject = entry.subject.clone();
            let sha = entry.sha.clone();
            let search_val = format!("{subject} {sha}");

            items.push(SelectionItem {
                name: subject.clone(),
                actions: vec![Box::new(move |tx3: &AppEventSender| {
                    tx3.send(AppEvent::CodexOp(Op::Review {
                        review_request: ReviewRequest {
                            target: ReviewTarget::Commit {
                                sha: sha.clone(),
                                title: Some(subject.clone()),
                            },
                            user_facing_hint: None,
                        },
                    }));
                })],
                dismiss_on_select: true,
                search_value: Some(search_val),
                ..Default::default()
            });
        }

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some("Select a commit to review".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Type to search commits".to_string()),
            ..Default::default()
        });
    }

    pub(crate) fn show_review_custom_prompt(&mut self) {
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "Custom review instructions".to_string(),
            "Type instructions and press Enter".to_string(),
            None,
            Box::new(move |prompt: String| {
                let trimmed = prompt.trim().to_string();
                if trimmed.is_empty() {
                    return;
                }
                tx.send(AppEvent::CodexOp(Op::Review {
                    review_request: ReviewRequest {
                        target: ReviewTarget::Custom {
                            instructions: trimmed,
                        },
                        user_facing_hint: None,
                    },
                }));
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    /// Programmatically submit a user text message as if typed in the
    /// composer. The text will be added to conversation history and sent to
    /// the agent.
    pub(crate) fn submit_text_message(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        if self.try_delegate_shortcut(&text) {
            return;
        }
        self.submit_user_message(text.into());
    }

    pub(crate) fn token_usage(&self) -> TokenUsage {
        self.token_info
            .as_ref()
            .map(|ti| ti.total_token_usage.clone())
            .unwrap_or_default()
    }

    pub(crate) fn conversation_id(&self) -> Option<ConversationId> {
        self.conversation_id
    }

    /// A lightweight status string for the sidebar, derived from existing UI state.
    /// 优先根据 TaskRunning 状态区分「运行中」和「就绪」，否则再回退到 Exec 活动。
    ///
    /// 这样即使为了 UI 效果临时隐藏底部状态指示器（例如流式最终答案已经落盘到历史，
    /// 但 Task 仍在进行中），会话栏仍然反映「运行中」直到整个任务真正结束
    ///（核心层发出 TaskComplete 事件）。
    pub(crate) fn sidebar_status(&self) -> String {
        // 只要底部 Pane 认为有任务在运行，就视为「运行中」，不依赖状态指示器是否可见。
        if self.bottom_pane.is_task_running() {
            return "运行中".to_string();
        }

        if self.queued_turn_pending_start {
            return "运行中".to_string();
        }

        // 检查 ExecCell/运行中命令
        let exec_active = self
            .active_cell
            .as_ref()
            .and_then(|c| c.as_any().downcast_ref::<ExecCell>())
            .map(crate::exec_cell::ExecCell::is_active)
            .unwrap_or(false);
        if exec_active || !self.running_commands.is_empty() {
            return "运行中".to_string();
        }

        // Fallback
        "就绪".to_string()
    }

    pub(crate) fn add_delegate_completion(
        &mut self,
        response: Option<&str>,
        duration_hint: Option<String>,
        label: &DelegateDisplayLabel,
    ) {
        let header = format!("{} completed", label.base_label);
        self.add_info_message(header, duration_hint);

        if label.depth > 0 {
            return;
        }

        let Some(text) = response.map(str::trim).filter(|s| !s.is_empty()) else {
            return;
        };

        self.flush_answer_stream_with_separator();
        self.flush_active_cell();

        let mut lines: Vec<ratatui::text::Line<'static>> = Vec::new();
        append_markdown(text, None, &mut lines);
        let cell = AgentMessageCell::new(lines, true);
        self.add_to_history(cell);
        self.request_redraw();
    }

    pub(crate) fn on_delegate_started(
        &mut self,
        run_id: &str,
        agent_id: &AgentId,
        prompt: &str,
        label: DelegateDisplayLabel,
        claim_status: bool,
        mode: DelegateSessionMode,
    ) {
        if claim_status {
            self.set_delegate_status_owner_internal(run_id, agent_id);
        }
        if label.depth == 0 {
            self.delegate_user_frames.clear();
            self.delegate_agent_frames.clear();
        }
        self.delegate_runs_with_stream.remove(run_id);
        self.delegate_run = Some(run_id.to_string());
        let trimmed = prompt.trim();
        let hint = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        let mut info_label = label.base_label;
        if mode == DelegateSessionMode::Detached {
            info_label = format!("{info_label} (detached)");
        }
        self.add_info_message(format!("{info_label}…"), hint);
        self.request_redraw();
    }

    pub(crate) fn on_delegate_delta(&mut self, run_id: &str, chunk: &str) {
        if self.delegate_run.as_deref() != Some(run_id) {
            self.delegate_run = Some(run_id.to_string());
        }
        self.delegate_runs_with_stream.insert(run_id.to_string());
        self.handle_streaming_delta(chunk.to_string());
    }

    pub(crate) fn on_delegate_completed(
        &mut self,
        run_id: &str,
        label: &DelegateDisplayLabel,
    ) -> bool {
        let had_stream = self.delegate_runs_with_stream.remove(run_id);
        if self.delegate_run.as_deref() == Some(run_id) {
            if had_stream {
                self.flush_answer_stream_with_separator();
                self.handle_stream_finished();
                self.app_event_tx.send(AppEvent::StopCommitAnimation);
            }
            self.delegate_run = None;
        }
        label.depth == 0 && had_stream
    }

    pub(crate) fn show_detached_completion_actions(
        &mut self,
        agent_id: &AgentId,
        run_id: &str,
        output: Option<&str>,
    ) {
        let mut items: Vec<SelectionItem> = Vec::new();
        if let Some(text) = output.map(str::trim).filter(|s| !s.is_empty()) {
            let preview = truncate_text(text, 200);
            let run_id_insert = run_id.to_string();
            let text_insert = text.to_string();
            items.push(SelectionItem {
                name: format!("Use output from #{}", agent_id.as_str()),
                description: Some(preview),
                is_current: false,
                actions: vec![Box::new(move |tx: &AppEventSender| {
                    tx.send(AppEvent::InsertUserTextMessage(text_insert.clone()));
                    tx.send(AppEvent::DismissDetachedRun(run_id_insert.clone()));
                })],
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let run_id_dismiss = run_id.to_string();
        items.push(SelectionItem {
            name: format!("Dismiss detached run #{}", agent_id.as_str()),
            description: Some("Remove this run from the list".to_string()),
            is_current: false,
            actions: vec![Box::new(move |tx: &AppEventSender| {
                tx.send(AppEvent::DismissDetachedRun(run_id_dismiss.clone()));
            })],
            dismiss_on_select: true,
            ..Default::default()
        });

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some(format!("#{} finished", agent_id.as_str())),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn on_delegate_failed(
        &mut self,
        run_id: &str,
        label: &DelegateDisplayLabel,
        error: &str,
    ) {
        let _ = self.on_delegate_completed(run_id, label);
        self.add_error_message(format!("{} failed: {error}", label.base_label));
    }

    pub(crate) fn notify_detached_completion(&mut self, label: &DelegateDisplayLabel) {
        self.notify(Notification::DetachedRunFinished {
            label: label.base_label.clone(),
        });
    }

    pub(crate) fn notify_detached_failure(&mut self, label: &DelegateDisplayLabel, error: &str) {
        self.notify(Notification::DetachedRunFailed {
            label: label.base_label.clone(),
            error: error.to_string(),
        });
    }

    pub(crate) fn set_delegate_status_owner(&mut self, run_id: &str, agent_id: &AgentId) {
        self.set_delegate_status_owner_internal(run_id, agent_id);
    }

    pub(crate) fn clear_delegate_status_owner(&mut self) {
        if self.delegate_status_owner.take().is_some() {
            if let Some(previous) = self.delegate_previous_status_header.take() {
                self.set_status_header(previous);
            }
            self.bottom_pane.set_task_running(false);
        }
    }

    fn set_delegate_status_owner_internal(&mut self, run_id: &str, agent_id: &AgentId) {
        let is_same = self.delegate_status_owner.as_deref() == Some(run_id);
        if !is_same && self.delegate_status_owner.is_none() {
            if self.delegate_previous_status_header.is_none() {
                self.delegate_previous_status_header = Some(self.current_status_header.clone());
            }
            if self.bottom_pane.status_widget().is_none() {
                self.bottom_pane.set_task_running(true);
            }
        }
        self.delegate_status_owner = Some(run_id.to_string());
        self.set_status_header(format!("Delegating to #{}", agent_id.as_str()));
    }

    fn try_delegate_shortcut(&mut self, _text: &str) -> bool {
        false
    }

    pub(crate) fn rollout_path(&self) -> Option<PathBuf> {
        self.current_rollout_path.clone()
    }

    #[cfg(test)]
    pub(crate) fn get_model_family(&self) -> ModelFamily {
        self.model_family.clone()
    }

    /// Return a reference to the widget's current config (includes any
    /// runtime overrides applied via TUI, e.g., model or approval policy).
    pub(crate) fn config_ref(&self) -> &Config {
        &self.config
    }

    pub(crate) fn clear_token_usage(&mut self) {
        self.token_info = None;
    }

    fn as_renderable(&self) -> RenderableItem<'_> {
        let active_cell_renderable = match &self.active_cell {
            Some(cell) => RenderableItem::Borrowed(cell).inset(Insets::tlbr(1, 0, 0, 0)),
            None => RenderableItem::Owned(Box::new(())),
        };
        let mut flex = FlexRenderable::new();
        flex.push(1, active_cell_renderable);
        flex.push(
            0,
            RenderableItem::Borrowed(&self.bottom_pane).inset(Insets::tlbr(1, 0, 0, 0)),
        );
        RenderableItem::Owned(Box::new(flex))
    }
}

impl Drop for ChatWidget {
    fn drop(&mut self) {
        self.stop_rate_limit_poller();
    }
}

impl Renderable for ChatWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.as_renderable().render(area, buf);
        self.last_rendered_width.set(Some(area.width as usize));
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.as_renderable().desired_height(width)
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.as_renderable().cursor_pos(area)
    }
}

enum Notification {
    AgentTurnComplete { response: String },
    ExecApprovalRequested { command: String },
    EditApprovalRequested { cwd: PathBuf, changes: Vec<PathBuf> },
    DetachedRunFinished { label: String },
    DetachedRunFailed { label: String, error: String },
    ElicitationRequested { server_name: String },
}

impl Notification {
    fn display(&self) -> String {
        match self {
            Notification::AgentTurnComplete { response } => {
                Notification::agent_turn_preview(response)
                    .unwrap_or_else(|| "Agent turn complete".to_string())
            }
            Notification::ExecApprovalRequested { command } => {
                format!("Approval requested: {}", truncate_text(command, 30))
            }
            Notification::EditApprovalRequested { cwd, changes } => {
                format!(
                    "Codex wants to edit {}",
                    if changes.len() == 1 {
                        #[allow(clippy::unwrap_used)]
                        display_path_for(changes.first().unwrap(), cwd)
                    } else {
                        format!("{} files", changes.len())
                    }
                )
            }
            Notification::DetachedRunFinished { label } => {
                format!("Detached delegate finished {label}")
            }
            Notification::DetachedRunFailed { label, error } => {
                let preview = truncate_text(error, 60);
                format!("Detached delegate failed {label}: {preview}")
            }
            Notification::ElicitationRequested { server_name } => {
                format!("Approval requested by {server_name}")
            }
        }
    }

    fn type_name(&self) -> &str {
        match self {
            Notification::AgentTurnComplete { .. } => "agent-turn-complete",
            Notification::ExecApprovalRequested { .. }
            | Notification::EditApprovalRequested { .. }
            | Notification::ElicitationRequested { .. } => "approval-requested",
            Notification::DetachedRunFinished { .. } => "detached-run-finished",
            Notification::DetachedRunFailed { .. } => "detached-run-failed",
        }
    }

    fn allowed_for(&self, settings: &Notifications) -> bool {
        match settings {
            Notifications::Enabled(enabled) => *enabled,
            Notifications::Custom(allowed) => allowed.iter().any(|a| a == self.type_name()),
        }
    }

    fn agent_turn_preview(response: &str) -> Option<String> {
        let mut normalized = String::new();
        for part in response.split_whitespace() {
            if !normalized.is_empty() {
                normalized.push(' ');
            }
            normalized.push_str(part);
        }
        let trimmed = normalized.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(truncate_text(trimmed, AGENT_NOTIFICATION_PREVIEW_GRAPHEMES))
        }
    }
}

const AGENT_NOTIFICATION_PREVIEW_GRAPHEMES: usize = 200;

const EXAMPLE_PROMPTS: [&str; 6] = [
    "Explain this codebase",
    "Summarize recent commits",
    "Implement {feature}",
    "Find and fix a bug in @filename",
    "Write tests for @filename",
    "Improve documentation in @filename",
];

// Extract the first bold (Markdown) element in the form **...** from `s`.
// Returns the inner text if found; otherwise `None`.
fn extract_first_bold(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < bytes.len() {
                if bytes[j] == b'*' && bytes[j + 1] == b'*' {
                    // Found closing **
                    let inner = &s[start..j];
                    let trimmed = inner.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    } else {
                        return None;
                    }
                }
                j += 1;
            }
            // No closing; stop searching (wait for more deltas)
            return None;
        }
        i += 1;
    }
    None
}

async fn fetch_rate_limits(base_url: String, auth: CodexAuth) -> Option<RateLimitSnapshot> {
    match BackendClient::from_auth(base_url, &auth).await {
        Ok(client) => match client.get_rate_limits().await {
            Ok(snapshot) => Some(snapshot),
            Err(err) => {
                debug!(error = ?err, "failed to fetch rate limits from /usage");
                None
            }
        },
        Err(err) => {
            debug!(error = ?err, "failed to construct backend client for rate limits");
            None
        }
    }
}

fn ralph_loop_help_text() -> &'static str {
    "🔄 **Ralph Loop** - 迭代式自我修正循环\n\n\
     **用法：**\n\
     • `/ralph-loop \"<PROMPT>\" -n <max-iterations> -c \"<PROMISE>\" -d <delay-seconds>`\n\
     • `/cancel-ralph`\n\n\
     **参数：**\n\
     • `-n, --max-iterations <num>` - 最大迭代次数（默认: 50，0 表示无限）\n\
     • `-c, --completion-promise <str>` - 完成信号（默认: \"COMPLETE\"）\n\
     • `-d, --delay <seconds>` - 每轮迭代前的延迟秒数（默认: 0）\n\
     • `-p, --prompt <text>` - 要重复的提示词\n\n\
     **示例：**\n\
     • `/ralph-loop \"Fix all tests. Output <promise>DONE</promise> when ALL tests pass.\" -n 30 -c DONE`\n\
     • `/ralph-loop \"Build API\" -n 20 -d 300` - 每轮迭代前等待 5 分钟\n\n\
     **工作方式：**\n\
     1. 运行一次 `/ralph-loop ...`\n\
     2. 每次任务完成后，如果没有检测到 `<promise>...</promise>`，会把 *同一条原始 prompt* 重新提交\n\
     3. 如果设置了 `-d` 延迟，会在下一轮迭代前等待指定秒数\n\
     4. 直到输出的 `<promise>TEXT</promise>` 里的 `TEXT` 与 `-c/--completion-promise` **完全匹配** 才停止\n\n\
     **注意：**\n\
     • `-n 0` 表示无限循环（推荐始终设置一个上限避免卡死）\n\
     • `-d` 延迟适用于需要时间解决问题的场景（如等待外部修复）\n\
     • completion-promise 是精确匹配（会做空白归一化），不要用多个不同承诺值\n"
}

fn ralph_state_file_path(cwd: &Path) -> PathBuf {
    cwd.join(".codex").join("ralph-loop.local.md")
}

fn save_ralph_state_file(cwd: &Path, state: &RalphLoopState) -> std::io::Result<()> {
    let state_dir = cwd.join(".codex");
    std::fs::create_dir_all(&state_dir)?;
    std::fs::write(
        ralph_state_file_path(cwd),
        create_ralph_state_file_content(state),
    )?;
    Ok(())
}

fn cleanup_ralph_state_file(cwd: &Path) -> std::io::Result<()> {
    let state_file = ralph_state_file_path(cwd);
    if state_file.exists() {
        std::fs::remove_file(state_file)?;
    }
    Ok(())
}

fn create_ralph_state_file_content(state: &RalphLoopState) -> String {
    format!(
        r#"---
active: true
iteration: {iteration}
max_iterations: {max_iterations}
completion_promise: {completion_promise}
delay_seconds: {delay_seconds}
started_at: {started_at}
---

{original_prompt}
"#,
        iteration = state.iteration,
        max_iterations = state.max_iterations,
        completion_promise = state.completion_promise.as_str(),
        delay_seconds = state.delay_seconds,
        started_at = state.started_at.as_str(),
        original_prompt = state.original_prompt.as_str(),
    )
}

fn truncate_string(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

fn check_completion_promise(output: &str, promise: &str) -> bool {
    extract_promise_text(output).is_some_and(|found| found == promise)
}

fn extract_promise_text(output: &str) -> Option<String> {
    let start = output.find("<promise>")?;
    let rest = &output[start + "<promise>".len()..];
    let end = rest.find("</promise>")?;
    Some(normalize_promise_text(&rest[..end]))
}

fn normalize_promise_text(text: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_space = false;

    for ch in text.chars() {
        if ch.is_whitespace() {
            if !last_was_space && !normalized.is_empty() {
                normalized.push(' ');
            }
            last_was_space = true;
        } else {
            normalized.push(ch);
            last_was_space = false;
        }
    }

    normalized.trim().to_string()
}

fn calculate_duration_seconds(started_at: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(started_at)
        .map(|start| {
            let duration = chrono::Utc::now().signed_duration_since(start);
            duration.num_milliseconds() as f64 / 1000.0
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
pub(crate) fn show_review_commit_picker_with_entries(
    chat: &mut ChatWidget,
    entries: Vec<codex_core::git_info::CommitLogEntry>,
) {
    let mut items: Vec<SelectionItem> = Vec::with_capacity(entries.len());
    for entry in entries {
        let subject = entry.subject.clone();
        let sha = entry.sha.clone();
        let search_val = format!("{subject} {sha}");

        items.push(SelectionItem {
            name: subject.clone(),
            actions: vec![Box::new(move |tx3: &AppEventSender| {
                tx3.send(AppEvent::CodexOp(Op::Review {
                    review_request: ReviewRequest {
                        target: ReviewTarget::Commit {
                            sha: sha.clone(),
                            title: Some(subject.clone()),
                        },
                        user_facing_hint: None,
                    },
                }));
            })],
            dismiss_on_select: true,
            search_value: Some(search_val),
            ..Default::default()
        });
    }

    chat.bottom_pane.show_selection_view(SelectionViewParams {
        title: Some("Select a commit to review".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Type to search commits".to_string()),
        ..Default::default()
    });
}

#[cfg(test)]
pub(crate) mod tests;
