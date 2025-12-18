use strum::IntoEnumIterator;
use strum_macros::AsRefStr;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

/// Commands that can be invoked by starting a message with a leading slash.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, EnumIter, AsRefStr, IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SlashCommand {
    // DO NOT ALPHA-SORT! Enum order is presentation order in the popup, so
    // more frequently used commands should be listed first.
    Model,
    Approvals,
    Skills,
    Review,
    New,
    Resume,
    Init,
    Tumix,
    TumixStop,
    Compact,
    Undo,
    Diff,
    OpenImage,
    RefImage,
    ClearRef,
    Mention,
    Agent,
    Status,
    Mcp,
    Logout,
    Quit,
    Exit,
    Feedback,
    Rollout,
    TestApproval,
}

impl SlashCommand {
    /// User-visible description shown in the popup.
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Feedback => "send logs to maintainers",
            SlashCommand::New => "start a new chat during a conversation",
            SlashCommand::Init => "create an AGENTS.md file with instructions for Codex",
            SlashCommand::Tumix => "run TUMIX multi-agent parallel execution (Round 1)",
            SlashCommand::TumixStop => "stop running TUMIX agents (optionally specify a session)",
            SlashCommand::Compact => "summarize conversation to prevent hitting the context limit",
            SlashCommand::Review => "review my current changes and find issues",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::Undo => "ask Codex to undo a turn",
            SlashCommand::Quit | SlashCommand::Exit => "exit Codex",
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::OpenImage => "open the most recently generated image",
            SlashCommand::RefImage => "set reference images for image models",
            SlashCommand::ClearRef => "clear active reference images",
            SlashCommand::Mention => "mention a file",
            SlashCommand::Agent => "switch into a delegated agent session",
            SlashCommand::Skills => "use skills to improve how Codex performs specific tasks",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::Model => "choose what model and reasoning effort to use",
            SlashCommand::Approvals => "choose what Codex can do without approval",
            SlashCommand::Mcp => "list configured MCP tools",
            SlashCommand::Logout => "log out of Codex",
            SlashCommand::Rollout => "print the rollout file path",
            SlashCommand::TestApproval => "test approval request",
        }
    }

    /// Whether this command accepts free-form arguments after the name.
    ///
    /// Commands that return `true` receive the raw argument string in
    /// `InputResult::CommandWithArgs` so their handlers can parse it in a
    /// context-aware way.
    pub fn accepts_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Tumix | SlashCommand::TumixStop | SlashCommand::RefImage
        )
    }

    /// Command string without the leading '/'. Provided for compatibility with
    /// existing code that expects a method named `command()`.
    pub fn command(self) -> &'static str {
        self.into()
    }

    /// Whether this command can be run while a task is in progress.
    pub fn available_during_task(self) -> bool {
        match self {
            SlashCommand::New
            | SlashCommand::Resume
            | SlashCommand::Init
            | SlashCommand::Tumix
            | SlashCommand::Compact
            | SlashCommand::Undo
            | SlashCommand::Model
            | SlashCommand::Approvals
            | SlashCommand::Review
            | SlashCommand::Logout => false,
            SlashCommand::Diff
            | SlashCommand::OpenImage
            | SlashCommand::RefImage
            | SlashCommand::ClearRef
            | SlashCommand::Mention
            | SlashCommand::Agent
            | SlashCommand::Skills
            | SlashCommand::Status
            | SlashCommand::Mcp
            | SlashCommand::TumixStop
            | SlashCommand::Feedback
            | SlashCommand::Quit
            | SlashCommand::Exit => true,
            SlashCommand::Rollout => true,
            SlashCommand::TestApproval => true,
        }
    }

    fn is_visible(self) -> bool {
        match self {
            SlashCommand::Rollout | SlashCommand::TestApproval => cfg!(debug_assertions),
            _ => true,
        }
    }
}

/// Return all built-in commands in a Vec paired with their command string.
pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    SlashCommand::iter()
        .filter(|command| command.is_visible())
        .map(|c| (c.command(), c))
        .collect()
}
