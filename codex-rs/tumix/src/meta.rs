//! Meta-agent that generates specialized agent configurations based on task complexity

use crate::AgentConfig;
use anyhow::Context;
use anyhow::Result;
use std::path::PathBuf;
use tokio::process::Command;

fn load_meta_prompt_template() -> Result<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let default_path = PathBuf::from(home).join(".codex/tumix/tumix-meta.md");
    let prompt_path = std::env::var("TUMIX_META_PROMPT_PATH")
        .map(PathBuf::from)
        .unwrap_or(default_path);

    std::fs::read_to_string(&prompt_path).with_context(|| {
        format!(
            "无法读取 TUMIX 元代理提示词，缺少文件：{}。请先创建模板。",
            prompt_path.display()
        )
    })
}

fn strip_front_matter(template: &str) -> &str {
    let text = template.trim_start_matches('\u{feff}');
    if let Some(rest) = text.strip_prefix("---\n") {
        if let Some(pos) = rest.find("\n---") {
            let body = &rest[pos + 4..];
            return body.trim_start_matches(['\n', '\r']);
        }
    } else if let Some(rest) = text.strip_prefix("---\r\n")
        && let Some(pos) = rest.find("\r\n---")
    {
        let body = &rest[pos + 5..];
        return body.trim_start_matches(['\n', '\r']);
    }
    text
}

/// Generate agent configurations via meta-agent (flexible count based on task)
pub async fn generate_agents(
    parent_session: &str,
    user_prompt: Option<String>,
) -> Result<Vec<AgentConfig>> {
    let task_desc = if let Some(ref prompt) = user_prompt {
        format!("用户任务：{}\n\n", prompt)
    } else {
        "（当前对话未额外提供用户提示）\n\n".to_string()
    };

    let meta_prompt_template = load_meta_prompt_template()?;
    let meta_prompt_body = strip_front_matter(&meta_prompt_template);
    let meta_prompt = meta_prompt_body.replace("$1", &task_desc);

    // Get codex binary path from environment or use default npm global installation
    let codex_bin = std::env::var("CODEX_BIN").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{}/.npm-global/bin/codex", home)
    });

    tracing::info!("Meta-agent: Using codex binary: {}", codex_bin);
    tracing::info!("Meta-agent: Session: {}", parent_session);
    if let Some(ref prompt) = user_prompt {
        tracing::info!("Meta-agent: User task: {}", prompt);
    }

    // Build the command arguments
    let args = vec![
        "exec",
        "--print-rollout-path",
        "--skip-git-repo-check",
        "--sandbox",
        "danger-full-access",
        "--model",
        "gpt-5-codex-high",
        "resume-clone",
        parent_session,
    ];

    // Build the full command line for debugging
    let full_command = format!(
        "{} {} \"{}\"",
        codex_bin,
        args.join(" "),
        meta_prompt
            .replace('\n', "\\n")
            .chars()
            .take(200)
            .collect::<String>()
    );

    tracing::info!("Meta-agent executing command:");
    tracing::info!("  {}", full_command);

    // Create .tumix directory if it doesn't exist
    let debug_dir = std::path::Path::new(".tumix");
    std::fs::create_dir_all(debug_dir).ok();

    // Save command to debug file
    let cmd_path = debug_dir.join("meta_agent_command.sh");
    let cmd_content = format!(
        "#!/bin/bash\n# Meta-agent command executed at {}\n\n{} \\\n  {} \\\n  \"{}\"\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        codex_bin,
        args.join(" \\\n  "),
        meta_prompt.replace('"', "\\\"")
    );
    let _ = std::fs::write(&cmd_path, cmd_content);
    tracing::info!("Meta-agent command saved to .tumix/meta_agent_command.sh");

    let output = Command::new(&codex_bin)
        .args(args)
        .arg(&meta_prompt)
        .output()
        .await
        .context("Failed to execute codex for meta-agent")?;

    // Save output to debug files (directory already created above)
    let stdout_path = debug_dir.join("meta_agent_stdout.txt");
    let stderr_path = debug_dir.join("meta_agent_stderr.txt");
    let _ = std::fs::write(&stdout_path, &output.stdout);
    let _ = std::fs::write(&stderr_path, &output.stderr);
    tracing::info!("Meta-agent output saved to .tumix/meta_agent_{{stdout,stderr}}.txt");

    tracing::info!("Meta-agent exit code: {:?}", output.status.code());
    tracing::debug!(
        "Meta-agent stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    tracing::debug!(
        "Meta-agent stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Check if we got valid JSON in stdout (don't fail on stderr warnings)
    let stdout = String::from_utf8_lossy(&output.stdout);

    // If stdout is empty or command failed with non-zero exit, then it's a real failure
    if !output.status.success() && stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("Meta-agent failed!");
        tracing::error!("Exit code: {:?}", output.status.code());
        tracing::error!("Stderr: {}", stderr);
        tracing::error!("Stdout: {}", stdout);
        anyhow::bail!("Meta-agent execution failed: {}", stderr);
    }

    // If we have stdout, try to extract JSON even if there were warnings in stderr
    if stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Meta-agent produced no output.\n\
             Exit code: {:?}\n\
             Stderr (first 500 chars): {}",
            output.status.code(),
            &stderr.chars().take(500).collect::<String>()
        );
    }

    // Extract JSON from output
    let json_str = extract_json(&stdout).context(format!(
        "Failed to extract JSON from meta-agent output.\n\
         Output saved to .tumix/meta_agent_stdout.txt for inspection.\n\
         First 500 chars: {}",
        &stdout.chars().take(500).collect::<String>()
    ))?;

    tracing::debug!(
        "Extracted JSON: {}",
        &json_str.chars().take(200).collect::<String>()
    );

    // Parse agent configs
    let agents: Vec<AgentConfig> = serde_json::from_str(&json_str).context(format!(
        "Failed to parse agent configurations.\nJSON: {}",
        &json_str.chars().take(500).collect::<String>()
    ))?;

    tracing::info!("Meta-agent returned {} agents", agents.len());

    // Validate
    if agents.is_empty() {
        anyhow::bail!(
            "Meta-agent returned 0 agents.\n\
             This likely means the agent didn't understand the task or failed to generate configs.\n\
             Check .tumix/meta_agent_stdout.txt for the full output."
        );
    }

    // Validate IDs are sequential
    for (i, agent) in agents.iter().enumerate() {
        let expected_id = format!("{:02}", i + 1);
        if agent.id != expected_id {
            tracing::warn!(
                "Agent {} has unexpected ID '{}', expected '{}'",
                i,
                agent.id,
                expected_id
            );
        }
    }

    tracing::debug!(
        "Generated agents: {:?}",
        agents.iter().map(|a| &a.name).collect::<Vec<_>>()
    );

    Ok(agents)
}

/// Extract JSON array from codex output
fn extract_json(text: &str) -> Result<String> {
    // Try to find ```json``` block
    if let Some(start) = text.find("```json")
        && let Some(end) = text[start + 7..].find("```")
    {
        let json = text[start + 7..start + 7 + end].trim();
        return Ok(json.to_string());
    }

    // Try to find ``` block
    if let Some(start) = text.find("```") {
        let after_marker = start + 3;
        // Skip language identifier if present
        let content_start = if let Some(newline) = text[after_marker..].find('\n') {
            after_marker + newline + 1
        } else {
            after_marker
        };

        if let Some(end) = text[content_start..].find("```") {
            let json = text[content_start..content_start + end].trim();
            return Ok(json.to_string());
        }
    }

    // Try to find JSON array directly
    if let Some(start) = text.find('[')
        && let Some(end) = text.rfind(']')
        && end > start
    {
        let json = text[start..=end].trim();
        // Basic validation
        if json.starts_with('[') && json.ends_with(']') {
            return Ok(json.to_string());
        }
    }

    anyhow::bail!("Could not find JSON array in output")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_with_markers() {
        let text = r#"
Sure, here are the agents:

```json
[
  {"id": "01", "name": "Test", "role": "Testing"}
]
```

Hope this helps!
        "#;

        let result = extract_json(text).unwrap();
        assert!(result.contains("\"id\""));
    }

    #[test]
    fn test_extract_json_plain() {
        let text = r#"
[
  {"id": "01", "name": "Test", "role": "Testing"}
]
        "#;

        let result = extract_json(text).unwrap();
        assert!(result.starts_with('['));
    }
}
