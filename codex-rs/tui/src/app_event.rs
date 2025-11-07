use std::path::PathBuf;

use codex_common::approval_presets::ApprovalPreset;
use codex_common::model_presets::ModelPreset;
use codex_core::protocol::ConversationPathResponseEvent;
use codex_core::protocol::Event;
use codex_file_search::FileMatch;
use codex_multi_agent::DelegateEvent;

use crate::bottom_pane::ApprovalRequest;
use crate::cxresume_picker_widget::PickerState;
use crate::history_cell::HistoryCell;

use codex_core::protocol::AskForApproval;
use codex_core::protocol::SandboxPolicy;
use codex_core::protocol_config_types::ReasoningEffort;

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum AppEvent {
    CodexEvent(Event),
    /// Event tagged with its source conversation id for routing/guarding
    CodexEventFor {
        conversation_id: String,
        event: Event,
    },

    /// Start a new session.
    NewSession,

    /// Resume an existing session from a saved rollout file.
    ResumeSession(PathBuf),

    /// Request to exit the application gracefully.
    ExitRequest,

    /// Forward an `Op` to the Agent. Using an `AppEvent` for this avoids
    /// bubbling channels through layers of widgets.
    CodexOp(codex_core::protocol::Op),

    /// Update emitted from the orchestrator about delegate progress/completion.
    DelegateUpdate(DelegateEvent),

    /// Request to launch a Tumix run using delegate-style UI wiring.
    TumixRunRequested {
        run_id: String,
        session_id: String,
        user_prompt: Option<String>,
        display_prompt: String,
    },

    /// Kick off an asynchronous file search for the given query (text after
    /// the `@`). Previous searches may be cancelled by the app layer so there
    /// is at most one in-flight search.
    StartFileSearch(String),
    /// Result of a completed asynchronous file search. The `query` echoes the
    /// original search term so the UI can decide whether the results are
    /// still relevant.
    FileSearchResult {
        query: String,
        matches: Vec<FileMatch>,
    },

    /// Result of computing a `/diff` command.
    DiffResult(String),

    InsertHistoryCell(Box<dyn HistoryCell>),

    StartCommitAnimation,
    StopCommitAnimation,
    CommitTick,

    /// Update the current reasoning effort in the running app and widget.
    UpdateReasoningEffort(Option<ReasoningEffort>),

    /// Update the current model slug in the running app and widget.
    UpdateModel(String),

    /// Persist the selected model and reasoning effort to the appropriate config.
    PersistModelSelection {
        model: String,
        effort: Option<ReasoningEffort>,
    },

    /// Open the reasoning selection popup after picking a model.
    OpenReasoningPopup {
        model: ModelPreset,
    },

    /// Open the confirmation prompt before enabling full access mode.
    OpenFullAccessConfirmation {
        preset: ApprovalPreset,
    },

    /// Show Windows Subsystem for Linux setup instructions for auto mode.
    ShowWindowsAutoModeInstructions,

    /// Update the current approval policy in the running app and widget.
    UpdateAskForApprovalPolicy(AskForApproval),

    /// Update the current sandbox policy in the running app and widget.
    UpdateSandboxPolicy(SandboxPolicy),

    /// Update whether the full access warning prompt has been acknowledged.
    UpdateFullAccessWarningAcknowledged(bool),

    /// Persist the acknowledgement flag for the full access warning prompt.
    PersistFullAccessWarningAcknowledged,

    /// Re-open the approval presets popup.
    OpenApprovalsPopup,

    /// Request to open the delegate session picker.
    OpenDelegatePicker,

    /// Switch into the provided delegate session.
    EnterDelegateSession(String),

    /// Return from the active delegate session to the main agent.
    ExitDelegateSession,

    /// Dismiss a detached delegate run from the registry.
    DismissDetachedRun(String),

    /// Inject text into the main composer as if the user typed it.
    InsertUserTextMessage(String),

    /// Forwarded conversation history snapshot from the current conversation.
    ConversationHistory(ConversationPathResponseEvent),

    /// Open the branch picker option from the review popup.
    OpenReviewBranchPicker(PathBuf),

    /// Open the commit picker option from the review popup.
    OpenReviewCommitPicker(PathBuf),

    /// Open the custom prompt option from the review popup.
    OpenReviewCustomPrompt,

    /// Open the approval popup.
    FullScreenApprovalRequest(ApprovalRequest),

    /// Triggered after a period without user interaction to prewarm cxresume state.
    CxresumeIdleCheck,

    /// Result of background cxresume prewarm.
    CxresumePrewarmReady(PickerState),

    /// Background cxresume prewarm failed.
    CxresumePrewarmFailed(String),

    /// Open the feedback note entry overlay after the user selects a category.
    OpenFeedbackNote {
        category: FeedbackCategory,
        include_logs: bool,
    },

    /// Open the upload consent popup for feedback after selecting a category.
    OpenFeedbackConsent {
        category: FeedbackCategory,
    },

    /// Update per-session runtime status (for UnifiedExec etc.).
    UpdateSessionStatus {
        session_id: String,
        status: String,
    },

    /// Update runtime status for the current active conversation (id inferred in App).
    UpdateCurrentSessionStatus {
        status: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FeedbackCategory {
    BadResult,
    GoodResult,
    Bug,
    Other,
}
