use anyhow::Result;
use anyhow::anyhow;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use shlex::split as shlex_split;
use ts_rs::TS;

/// Slash commands that can be executed in the Codex session
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SlashCommand {
    /// Activate Ralph Loop mode
    RalphLoop(RalphLoopCommand),

    /// Cancel active Ralph Loop
    CancelRalph,

    /// Show help information
    Help,
}

/// Ralph Loop command with options
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RalphLoopCommand {
    /// Maximum iterations (default: 50)
    pub max_iterations: u32,

    /// Completion promise to detect (default: "COMPLETE")
    pub completion_promise: String,

    /// Optional: specific prompt to repeat
    /// If None, use the last user message
    pub prompt: Option<String>,

    /// Delay in seconds before starting next iteration (default: 0)
    /// Useful when errors occur and need time to be resolved
    pub delay_seconds: u64,
}

impl Default for RalphLoopCommand {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            completion_promise: "COMPLETE".to_string(),
            prompt: None,
            delay_seconds: 0,
        }
    }
}

impl SlashCommand {
    /// Parse a slash command from user input
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();

        if !input.starts_with('/') {
            return Err(anyhow!("Not a slash command (must start with /)"));
        }

        let parts: Vec<String> = shlex_split(input)
            .unwrap_or_else(|| input.split_whitespace().map(ToString::to_string).collect());

        if parts.is_empty() {
            return Err(anyhow!("Empty command"));
        }

        match parts[0].as_str() {
            "/ralph-loop" => {
                let cmd = parse_ralph_loop_args(&parts[1..])?;
                Ok(SlashCommand::RalphLoop(cmd))
            }

            "/cancel-ralph" => Ok(SlashCommand::CancelRalph),

            "/help" => Ok(SlashCommand::Help),

            _ => {
                let command = parts[0].as_str();
                Err(anyhow!("Unknown command: {command}"))
            }
        }
    }

    /// Check if input is a slash command
    pub fn is_slash_command(input: &str) -> bool {
        input.trim().starts_with('/')
    }
}

fn parse_ralph_loop_args(args: &[String]) -> Result<RalphLoopCommand> {
    // Codex default: always set a finite limit unless the user explicitly opts
    // into an unlimited loop with `--max-iterations 0`.
    let mut max_iterations = 50;
    let mut completion_promise = "COMPLETE".to_string();
    let mut prompt = None;
    let mut delay_seconds = 0u64;
    let mut positional_prompt_parts: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--max-iterations" | "-n" => {
                i += 1;
                if i < args.len() {
                    let value = args[i].as_str();
                    max_iterations = value
                        .parse()
                        .map_err(|_| anyhow!("Invalid max-iterations value: {value}"))?;
                } else {
                    return Err(anyhow!("--max-iterations requires a value"));
                }
            }

            "--completion-promise" | "-c" => {
                i += 1;
                if i < args.len() {
                    completion_promise = args[i].clone();
                } else {
                    return Err(anyhow!("--completion-promise requires a value"));
                }
            }

            "--delay" | "-d" => {
                i += 1;
                if i < args.len() {
                    let value = args[i].as_str();
                    delay_seconds = value
                        .parse()
                        .map_err(|_| anyhow!("Invalid delay value: {value}"))?;
                } else {
                    return Err(anyhow!("--delay requires a value"));
                }
            }

            "--prompt" | "-p" => {
                i += 1;
                if i < args.len() {
                    let mut prompt_parts = Vec::new();
                    while i < args.len() {
                        let token = args[i].as_str();
                        if is_ralph_loop_option(token) {
                            if prompt_parts.is_empty() {
                                return Err(anyhow!("--prompt requires a value"));
                            }
                            i -= 1;
                            break;
                        }

                        prompt_parts.push(args[i].clone());
                        i += 1;
                    }

                    if prompt_parts.is_empty() {
                        return Err(anyhow!("--prompt requires a value"));
                    }

                    prompt = Some(prompt_parts.join(" "));
                } else {
                    return Err(anyhow!("--prompt requires a value"));
                }
            }

            _ => {
                let option = args[i].as_str();
                if option.starts_with('-') {
                    return Err(anyhow!("Unknown option: {option}"));
                }

                positional_prompt_parts.push(args[i].clone());
            }
        }
        i += 1;
    }

    if prompt.is_some() && !positional_prompt_parts.is_empty() {
        return Err(anyhow!(
            "Provide the prompt either via --prompt/-p or as positional arguments, not both"
        ));
    }

    let prompt = prompt.or_else(|| {
        if positional_prompt_parts.is_empty() {
            None
        } else {
            Some(positional_prompt_parts.join(" "))
        }
    });

    Ok(RalphLoopCommand {
        max_iterations,
        completion_promise,
        prompt,
        delay_seconds,
    })
}

fn is_ralph_loop_option(token: &str) -> bool {
    matches!(
        token,
        "--max-iterations" | "-n" | "--completion-promise" | "-c" | "--prompt" | "-p" | "--delay" | "-d"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_parse_ralph_loop_basic() {
        let cmd = SlashCommand::parse("/ralph-loop").unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.max_iterations, 50);
                assert_eq!(ralph.completion_promise, "COMPLETE");
                assert_eq!(ralph.prompt, None);
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }

    #[test]
    fn test_parse_ralph_loop_with_options() {
        let cmd = SlashCommand::parse("/ralph-loop --max-iterations 30 --completion-promise DONE")
            .unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.max_iterations, 30);
                assert_eq!(ralph.completion_promise, "DONE");
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }

    #[test]
    fn test_parse_ralph_loop_with_prompt() {
        let cmd = SlashCommand::parse("/ralph-loop --prompt Build REST API").unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.prompt, Some("Build REST API".to_string()));
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }

    #[test]
    fn test_parse_ralph_loop_with_positional_prompt() {
        let cmd = SlashCommand::parse("/ralph-loop Build REST API -n 10 -c DONE").unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.prompt, Some("Build REST API".to_string()));
                assert_eq!(ralph.max_iterations, 10);
                assert_eq!(ralph.completion_promise, "DONE");
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }

    #[test]
    fn test_parse_ralph_loop_with_prompt_and_options_after() {
        let cmd = SlashCommand::parse("/ralph-loop --prompt Build REST API -n 10 -c DONE").unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.prompt, Some("Build REST API".to_string()));
                assert_eq!(ralph.max_iterations, 10);
                assert_eq!(ralph.completion_promise, "DONE");
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }

    #[test]
    fn test_parse_cancel_ralph() {
        let cmd = SlashCommand::parse("/cancel-ralph").unwrap();
        assert_eq!(cmd, SlashCommand::CancelRalph);
    }

    #[test]
    fn test_parse_help() {
        let cmd = SlashCommand::parse("/help").unwrap();
        assert_eq!(cmd, SlashCommand::Help);
    }

    #[test]
    fn test_is_slash_command() {
        assert!(SlashCommand::is_slash_command("/ralph-loop"));
        assert!(SlashCommand::is_slash_command("  /help  "));
        assert!(!SlashCommand::is_slash_command("regular message"));
        assert!(!SlashCommand::is_slash_command(""));
    }

    #[test]
    fn test_parse_unknown_command() {
        let result = SlashCommand::parse("/unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_not_slash_command() {
        let result = SlashCommand::parse("regular message");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_ralph_loop_with_delay() {
        let cmd = SlashCommand::parse("/ralph-loop --delay 300").unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.delay_seconds, 300);
                assert_eq!(ralph.max_iterations, 50); // default
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }

    #[test]
    fn test_parse_ralph_loop_with_delay_short() {
        let cmd = SlashCommand::parse("/ralph-loop -d 60 -n 20").unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.delay_seconds, 60);
                assert_eq!(ralph.max_iterations, 20);
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }

    #[test]
    fn test_parse_ralph_loop_with_all_options() {
        let cmd =
            SlashCommand::parse("/ralph-loop \"Build API\" -n 30 -c DONE -d 300").unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.prompt, Some("Build API".to_string()));
                assert_eq!(ralph.max_iterations, 30);
                assert_eq!(ralph.completion_promise, "DONE");
                assert_eq!(ralph.delay_seconds, 300);
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }

    #[test]
    fn test_parse_ralph_loop_default_delay() {
        let cmd = SlashCommand::parse("/ralph-loop -n 10").unwrap();
        match cmd {
            SlashCommand::RalphLoop(ralph) => {
                assert_eq!(ralph.delay_seconds, 0); // default is 0
            }
            _ => panic!("Expected RalphLoop command"),
        }
    }
}
