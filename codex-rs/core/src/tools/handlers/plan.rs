use crate::client_common::tools::ResponsesApiTool;
use crate::client_common::tools::ToolSpec;
use crate::codex::Session;
use crate::codex::TurnContext;
use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use crate::tools::spec::JsonSchema;
use async_trait::async_trait;
use codex_protocol::plan_tool::UpdatePlanArgs;
use codex_protocol::protocol::EventMsg;
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub struct PlanHandler;

pub static PLAN_TOOL: LazyLock<ToolSpec> = LazyLock::new(|| {
    let mut plan_item_props = BTreeMap::new();
    plan_item_props.insert("step".to_string(), JsonSchema::String { description: None });
    plan_item_props.insert(
        "status".to_string(),
        JsonSchema::String {
            description: Some("One of: pending, in_progress, completed".to_string()),
        },
    );

    let plan_items_schema = JsonSchema::Array {
        description: Some("The list of steps".to_string()),
        items: Box::new(JsonSchema::Object {
            properties: plan_item_props,
            required: Some(vec!["step".to_string(), "status".to_string()]),
            additional_properties: Some(false.into()),
        }),
    };

    let mut properties = BTreeMap::new();
    properties.insert(
        "explanation".to_string(),
        JsonSchema::String { description: None },
    );
    properties.insert("plan".to_string(), plan_items_schema);

    ToolSpec::Function(ResponsesApiTool {
        name: "update_plan".to_string(),
        description: r#"Updates the task plan.
Provide an optional explanation and a list of plan items, each with a step and status.
At most one step can be in_progress at a time.
"#
        .to_string(),
        strict: false,
        parameters: JsonSchema::Object {
            properties,
            required: Some(vec!["plan".to_string()]),
            additional_properties: Some(false.into()),
        },
    })
});

#[async_trait]
impl ToolHandler for PlanHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "update_plan handler received unsupported payload".to_string(),
                ));
            }
        };

        let content =
            handle_update_plan(session.as_ref(), turn.as_ref(), arguments, call_id).await?;

        Ok(ToolOutput::Function {
            content,
            content_items: None,
            success: Some(true),
        })
    }
}

/// This function doesn't do anything useful. However, it gives the model a structured way to record its plan that clients can read and render.
/// So it's the _inputs_ to this function that are useful to clients, not the outputs and neither are actually useful for the model other
/// than forcing it to come up and document a plan (TBD how that affects performance).
pub(crate) async fn handle_update_plan(
    session: &Session,
    turn_context: &TurnContext,
    arguments: String,
    _call_id: String,
) -> Result<String, FunctionCallError> {
    let args = parse_update_plan_arguments(&arguments)?;
    session
        .send_event(turn_context, EventMsg::PlanUpdate(args))
        .await;
    Ok("Plan updated".to_string())
}

fn parse_update_plan_arguments(arguments: &str) -> Result<UpdatePlanArgs, FunctionCallError> {
    serde_json::from_str::<UpdatePlanArgs>(arguments).map_err(|e| {
        FunctionCallError::RespondToModel(format!("failed to parse function arguments: {e}"))
    })
}

/// Intercepts shell commands that look like `update_plan --explanation "..." --plan '[...]'`
/// and handles them as proper function calls instead.
///
/// This is needed because some models (like Gemini) sometimes output tool calls as shell commands.
/// Returns `Some(ToolOutput)` if the command was intercepted, `None` otherwise.
pub(crate) async fn intercept_update_plan(
    command: &[String],
    session: &Session,
    turn: &TurnContext,
    tool_name: &str,
) -> Result<Option<ToolOutput>, FunctionCallError> {
    // Check if this looks like an update_plan command
    if command.is_empty() {
        return Ok(None);
    }

    let cmd = &command[0];
    if cmd != "update_plan" {
        return Ok(None);
    }

    // Parse the command-line style arguments into JSON
    let arguments = match parse_update_plan_shell_args(command) {
        Some(args) => args,
        None => return Ok(None),
    };

    // Log a warning that the model used shell command instead of function call
    session
        .record_model_warning(
            format!("update_plan was requested via {tool_name}. Use the update_plan tool instead of shell command."),
            turn,
        )
        .await;

    // Handle the update_plan as if it were a proper function call
    let content = handle_update_plan(session, turn, arguments, "intercepted".to_string()).await?;

    Ok(Some(ToolOutput::Function {
        content,
        content_items: None,
        success: Some(true),
    }))
}

/// Parse shell-style update_plan arguments into JSON string.
/// Handles formats like: `update_plan --explanation "text" --plan '[{"step": "...", "status": "..."}]'`
fn parse_update_plan_shell_args(command: &[String]) -> Option<String> {
    if command.is_empty() || command[0] != "update_plan" {
        return None;
    }

    let mut explanation: Option<String> = None;
    let mut plan: Option<String> = None;

    let mut i = 1;
    while i < command.len() {
        let arg = &command[i];
        if arg == "--explanation" && i + 1 < command.len() {
            explanation = Some(command[i + 1].clone());
            i += 2;
        } else if arg == "--plan" && i + 1 < command.len() {
            plan = Some(command[i + 1].clone());
            i += 2;
        } else if arg.starts_with("--explanation=") {
            explanation = Some(arg.strip_prefix("--explanation=")?.to_string());
            i += 1;
        } else if arg.starts_with("--plan=") {
            plan = Some(arg.strip_prefix("--plan=")?.to_string());
            i += 1;
        } else {
            // Unknown argument, might be the whole thing as a single string
            // Try to parse it as JSON directly
            if arg.contains("--explanation") || arg.contains("--plan") {
                // It's a combined string, try to extract parts
                if let Some(json) = try_parse_combined_args(arg) {
                    return Some(json);
                }
            }
            i += 1;
        }
    }

    // Build JSON from parsed arguments
    let plan_value = plan?;

    let mut json_obj = serde_json::Map::new();
    if let Some(exp) = explanation {
        json_obj.insert("explanation".to_string(), serde_json::Value::String(exp));
    }

    // Try to parse plan as JSON array
    if let Ok(plan_array) = serde_json::from_str::<serde_json::Value>(&plan_value) {
        json_obj.insert("plan".to_string(), plan_array);
    } else {
        return None;
    }

    serde_json::to_string(&json_obj).ok()
}

/// Try to parse a combined argument string that contains both --explanation and --plan
fn try_parse_combined_args(arg: &str) -> Option<String> {
    // Handle case where the entire command is passed as a single string
    // e.g., 'update_plan --explanation "..." --plan '[...]''

    let mut explanation: Option<String> = None;
    let mut plan: Option<String> = None;

    // Try to find --explanation
    if let Some(exp_start) = arg.find("--explanation") {
        let after_exp = &arg[exp_start + "--explanation".len()..];
        let after_exp = after_exp.trim_start_matches([' ', '=']);

        // Find the value (quoted string)
        if after_exp.starts_with('"')
            && let Some(end) = after_exp[1..].find('"')
        {
            explanation = Some(after_exp[1..=end].to_string());
        }
    }

    // Try to find --plan
    if let Some(plan_start) = arg.find("--plan") {
        let after_plan = &arg[plan_start + "--plan".len()..];
        let after_plan = after_plan.trim_start_matches([' ', '=']);

        // Find the JSON array (starts with '[')
        if let Some(bracket_start) = after_plan.find('[') {
            let json_part = &after_plan[bracket_start..];
            // Find matching closing bracket
            let mut depth = 0;
            let mut end_idx = 0;
            for (idx, ch) in json_part.chars().enumerate() {
                match ch {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            end_idx = idx + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if end_idx > 0 {
                plan = Some(json_part[..end_idx].to_string());
            }
        }
    }

    // Build JSON
    let plan_value = plan?;

    let mut json_obj = serde_json::Map::new();
    if let Some(exp) = explanation {
        json_obj.insert("explanation".to_string(), serde_json::Value::String(exp));
    }

    if let Ok(plan_array) = serde_json::from_str::<serde_json::Value>(&plan_value) {
        json_obj.insert("plan".to_string(), plan_array);
    } else {
        return None;
    }

    serde_json::to_string(&json_obj).ok()
}
