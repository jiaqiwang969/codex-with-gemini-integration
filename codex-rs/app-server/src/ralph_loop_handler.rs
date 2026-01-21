use crate::codex_message_processor::TurnSummary;
use crate::codex_message_processor::TurnSummaryStore;
use crate::outgoing_message::OutgoingMessageSender;
use crate::outgoing_message::OutgoingNotification;
use crate::ralph_loop_utils::cleanup_ralph_state_file;
use crate::ralph_loop_utils::save_ralph_state_file;
use crate::ralph_loop_utils::truncate_string;
use codex_protocol::ConversationId;
use codex_protocol::protocol::AgentMessageEvent;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RalphCompletionReason;
use codex_protocol::protocol::RalphLoopCompleteEvent;
use codex_protocol::protocol::RalphLoopState;
use codex_protocol::protocol::RalphLoopStatusEvent;
use codex_protocol::slash_commands::RalphLoopCommand;
use codex_protocol::slash_commands::SlashCommand;
use std::sync::Arc;
use tracing::info;
use tracing::warn;

/// Handle slash command input from user
pub(crate) async fn handle_slash_command(
    input: &str,
    conversation_id: ConversationId,
    turn_summary_store: &TurnSummaryStore,
    outgoing: &Arc<OutgoingMessageSender>,
) -> Result<bool, anyhow::Error> {
    // Check if it's a slash command
    if !SlashCommand::is_slash_command(input) {
        return Ok(false);
    }

    // Align with typical CLI behavior (and the upstream plugin): if the user
    // enters `/ralph-loop` with no args, show usage/help instead of trying to
    // start a loop.
    if input.trim() == "/ralph-loop" {
        send_help_message(outgoing, conversation_id).await;
        return Ok(true);
    }

    // Parse the command
    let command = match SlashCommand::parse(input) {
        Ok(cmd) => cmd,
        Err(e) => {
            warn!("Failed to parse slash command: {e}");
            let message = format!("Invalid command: {e}");
            send_error_message(outgoing, conversation_id, &message).await;
            return Ok(true);
        }
    };

    // Handle the command
    match command {
        SlashCommand::RalphLoop(cmd) => {
            handle_ralph_loop_activate(cmd, conversation_id, turn_summary_store, outgoing).await?;
        }
        SlashCommand::CancelRalph => {
            handle_ralph_loop_cancel(conversation_id, turn_summary_store, outgoing).await?;
        }
        SlashCommand::Help => {
            send_help_message(outgoing, conversation_id).await;
        }
    };

    Ok(true)
}

/// Activate Ralph Loop mode
async fn handle_ralph_loop_activate(
    cmd: RalphLoopCommand,
    conversation_id: ConversationId,
    turn_summary_store: &TurnSummaryStore,
    outgoing: &Arc<OutgoingMessageSender>,
) -> Result<(), anyhow::Error> {
    info!("Activating Ralph Loop for conversation {conversation_id}");

    // Get the prompt to repeat
    let prompt = if let Some(prompt) = cmd.prompt {
        prompt
    } else if let Some(prompt) = {
        let store = turn_summary_store.lock().await;
        store
            .get(&conversation_id)
            .and_then(|summary| summary.last_user_message.clone())
    } {
        prompt
    } else {
        send_error_message(
            outgoing,
            conversation_id,
            "Please specify a prompt (positional or with --prompt), or send a message first.\n\
             Example: /ralph-loop \"Build API. Output <promise>COMPLETE</promise> when done.\" -n 30",
        )
        .await;
        return Ok(());
    };

    // Create Ralph state
    let ralph_state = RalphLoopState::new(
        prompt.clone(),
        cmd.max_iterations,
        cmd.completion_promise.clone(),
    );

    // Store in turn summary
    {
        let mut store = turn_summary_store.lock().await;
        let summary = store
            .entry(conversation_id)
            .or_insert_with(TurnSummary::default);
        summary.ralph_loop_state = Some(ralph_state.clone());
    }

    if let Err(err) = save_ralph_state_file(&ralph_state).await {
        warn!("Failed to save Ralph state file: {err}");
    }

    // Send activation notification
    let completion_promise = cmd.completion_promise.as_str();
    let max_iterations = cmd.max_iterations;
    let truncated_prompt = truncate_string(&prompt, 100);
    let max_iterations_label = if max_iterations == 0 {
        "unlimited".to_string()
    } else {
        max_iterations.to_string()
    };
    let status_event = RalphLoopStatusEvent {
        iteration: ralph_state.iteration,
        max_iterations,
        message: format!(
            "🔄 Ralph Loop activated!\n\
             \n\
             Iteration: 1\n\
             Max iterations: {max_iterations_label}\n\
             Completion promise: <promise>{completion_promise}</promise>\n\
             \n\
             The loop is now active. When you try to exit, the SAME PROMPT will be\n\
             fed back to you. You'll see your previous work in files, creating a\n\
             self-referential loop where you iteratively improve on the same task.\n\
             \n\
             To monitor: cat .codex/ralph-loop.local.md\n\
             \n\
             ⚠️  WARNING: Set --max-iterations to prevent infinite loops!\n\
             \n\
             🔄 {truncated_prompt}",
        ),
    };

    send_ralph_status_notification(outgoing, conversation_id, status_event).await;

    info!("Ralph Loop activated successfully");

    Ok(())
}

/// Cancel active Ralph Loop
async fn handle_ralph_loop_cancel(
    conversation_id: ConversationId,
    turn_summary_store: &TurnSummaryStore,
    outgoing: &Arc<OutgoingMessageSender>,
) -> Result<(), anyhow::Error> {
    info!("Cancelling Ralph Loop for conversation {conversation_id}");

    let mut store = turn_summary_store.lock().await;

    let Some(summary) = store.get_mut(&conversation_id) else {
        send_error_message(
            outgoing,
            conversation_id,
            "⚠️  No active Ralph Loop to cancel",
        )
        .await;
        return Ok(());
    };

    let Some(ralph_state) = summary.ralph_loop_state.as_ref() else {
        send_error_message(
            outgoing,
            conversation_id,
            "⚠️  No active Ralph Loop to cancel",
        )
        .await;
        return Ok(());
    };

    // Calculate duration
    let started_at = chrono::DateTime::parse_from_rfc3339(&ralph_state.started_at)
        .unwrap_or_else(|_| chrono::Utc::now().into());
    let duration = chrono::Utc::now().signed_duration_since(started_at);

    // Send completion event
    let complete_event = RalphLoopCompleteEvent {
        total_iterations: ralph_state.iteration,
        completion_reason: RalphCompletionReason::UserInterrupt,
        duration_seconds: duration.num_milliseconds() as f64 / 1000.0,
    };

    send_ralph_complete_notification(outgoing, conversation_id, complete_event).await;

    // Clear the state
    summary.ralph_loop_state = None;

    if let Err(err) = cleanup_ralph_state_file().await {
        warn!("Failed to cleanup Ralph state file: {err}");
    }

    info!("Ralph Loop cancelled successfully");

    Ok(())
}

pub(crate) async fn send_ralph_status_notification(
    outgoing: &Arc<OutgoingMessageSender>,
    conversation_id: ConversationId,
    event: RalphLoopStatusEvent,
) {
    let message = event.message.clone();
    send_event_notification(outgoing, conversation_id, EventMsg::RalphLoopStatus(event)).await;
    info!("Ralph Loop Status: {message}");
}

pub(crate) async fn send_ralph_complete_notification(
    outgoing: &Arc<OutgoingMessageSender>,
    conversation_id: ConversationId,
    event: RalphLoopCompleteEvent,
) {
    let total_iterations = event.total_iterations;
    let completion_reason = event.completion_reason.clone();
    let duration_seconds = event.duration_seconds;
    send_event_notification(
        outgoing,
        conversation_id,
        EventMsg::RalphLoopComplete(event),
    )
    .await;
    info!(
        "Ralph Loop Complete: {total_iterations} iterations, reason: {completion_reason:?}, duration: {duration_seconds:.2}s",
    );
}

async fn send_error_message(
    outgoing: &Arc<OutgoingMessageSender>,
    conversation_id: ConversationId,
    message: &str,
) {
    let event = ErrorEvent {
        message: message.to_string(),
        codex_error_info: None,
    };
    send_event_notification(outgoing, conversation_id, EventMsg::Error(event)).await;
    warn!("Error message: {message}");
}

async fn send_help_message(outgoing: &Arc<OutgoingMessageSender>, conversation_id: ConversationId) {
    let help_text = r#"
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
                    Ralph Loop Commands
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/ralph-loop [options]
    Activate Ralph Loop to automatically repeat the task until completion.

    The loop works by intercepting task completion and re-injecting the SAME
    prompt. You'll see your previous work in files and git history, creating
    a self-referential feedback loop for iterative improvement.

    Options:
      [PROMPT...]                   The prompt to repeat (positional)
      --prompt, -p <text>           The prompt to repeat (alternative to positional args)
      --max-iterations, -n <num>    Maximum iterations (default: 50; 0 = unlimited)
      --completion-promise, -c <str> Completion signal (default: "COMPLETE")

    Completion Signal:
      Use <promise>TEXT</promise> tags in your output to signal completion.
      Example: "All tests passing. <promise>COMPLETE</promise>"

    Examples:
      /ralph-loop "Build REST API with tests. Output <promise>COMPLETE</promise> when done." -n 30

      /ralph-loop -p "Fix all TypeScript errors. Run 'npm run build' to verify. Output <promise>DONE</promise> when build succeeds." -n 20 -c "DONE"

      /ralph-loop --prompt "Implement feature X following TDD:
      1. Write failing tests
      2. Implement feature
      3. Run tests and fix failures
      4. Refactor if needed
      5. Output <promise>COMPLETE</promise> when all tests pass" -n 50

/cancel-ralph
    Cancel the active Ralph Loop.

    Example:
      /cancel-ralph

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
                    Best Practices
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

1. Clear Completion Criteria
   ✅ "All tests passing (coverage > 80%). <promise>COMPLETE</promise>"
   ❌ "Make it good"

2. Incremental Goals
   ✅ "Phase 1: Auth, Phase 2: API, Phase 3: Tests"
   ❌ "Build entire e-commerce platform"

3. Self-Correction Instructions
   ✅ "Run tests after each change. Fix failures before continuing."
   ❌ "Write code"

4. Safety Limits
   ✅ Always set --max-iterations
   ❌ Infinite loops without limits

5. Use <promise> Tags
   ✅ "Task complete. <promise>COMPLETE</promise>"
   ❌ "COMPLETE" (may match prematurely)

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

CRITICAL RULE: Only output the completion promise when the statement is
completely and unequivocally TRUE. Do not output false promises to escape
the loop, even if you think you're stuck. The loop is designed to continue
until genuine completion.

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
"#;

    let event = AgentMessageEvent {
        message: help_text.to_string(),
    };
    send_event_notification(outgoing, conversation_id, EventMsg::AgentMessage(event)).await;
    info!("Help: {help_text}");
}

async fn send_event_notification(
    outgoing: &Arc<OutgoingMessageSender>,
    conversation_id: ConversationId,
    msg: EventMsg,
) {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let event_id = format!("ralph-loop-{timestamp_ms}");
    let event = Event { id: event_id, msg };
    let msg_name = event.msg.to_string();
    let method = format!("codex/event/{msg_name}");

    let mut params = match serde_json::to_value(&event) {
        Ok(serde_json::Value::Object(map)) => map,
        Ok(_) => {
            warn!("Ralph loop event did not serialize to an object");
            return;
        }
        Err(err) => {
            warn!("Failed to serialize Ralph loop event: {err}");
            return;
        }
    };

    params.insert(
        "conversationId".to_string(),
        conversation_id.to_string().into(),
    );

    outgoing
        .send_notification(OutgoingNotification {
            method,
            params: Some(params.into()),
        })
        .await;
}
