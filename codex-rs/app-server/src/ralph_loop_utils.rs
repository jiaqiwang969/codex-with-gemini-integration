// Ralph Loop 辅助函数 - 基于 Claude Code 的实现

use codex_core::CodexConversation;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::RolloutLine;
use std::sync::Arc;

/// 从 conversation 获取最后的 agent 输出
///
/// 基于 Claude Code 的实现，从转录文件（rollout）中提取最后一条 agent 消息
pub(crate) async fn get_last_agent_output(conversation: &Arc<CodexConversation>) -> String {
    // 获取 rollout 路径
    let rollout_path = conversation.rollout_path();

    // 尝试读取 rollout 文件
    match tokio::fs::read_to_string(&rollout_path).await {
        Ok(content) => {
            // Rollout 是 JSONL 格式（每行一个 RolloutLine）
            // 从末尾倒序查找最后一条 agent 消息
            for line in content.lines().rev() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let Ok(rollout_line) = serde_json::from_str::<RolloutLine>(trimmed) else {
                    continue;
                };

                match rollout_line.item {
                    RolloutItem::EventMsg(EventMsg::AgentMessage(event)) => {
                        return event.message;
                    }
                    RolloutItem::ResponseItem(item) => {
                        if let Some(text) = agent_text_from_response_item(&item) {
                            return text;
                        }
                    }
                    _ => {}
                }
            }
            String::new()
        }
        Err(e) => {
            tracing::warn!("Failed to read rollout file: {e}");
            String::new()
        }
    }
}

fn agent_text_from_response_item(item: &ResponseItem) -> Option<String> {
    let ResponseItem::Message { role, content, .. } = item else {
        return None;
    };

    if role != "assistant" {
        return None;
    }

    let text = content
        .iter()
        .filter_map(|item| match item {
            ContentItem::OutputText { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();

    if text.is_empty() { None } else { Some(text) }
}

/// 检查完成承诺（使用 <promise> 标签，类似 Claude Code）
///
/// Claude Code 使用 <promise>TEXT</promise> 格式
/// 我们与官方实现保持一致：只匹配 `<promise>...</promise>` 标签内容。
pub(crate) fn check_completion_promise(output: &str, promise: &str) -> bool {
    let Some(found) = extract_promise_text(output) else {
        return false;
    };

    found == promise
}

/// 计算持续时间（秒）
pub(crate) fn calculate_duration(started_at: &str) -> f64 {
    if let Ok(start) = chrono::DateTime::parse_from_rfc3339(started_at) {
        let now = chrono::Utc::now();
        let duration = now.signed_duration_since(start);
        duration.num_milliseconds() as f64 / 1000.0
    } else {
        0.0
    }
}

/// 截断字符串
pub(crate) fn truncate_string(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{truncated}...")
    }
}

fn extract_promise_text(output: &str) -> Option<String> {
    let start = output.find("<promise>")?;
    let rest = &output[start + "<promise>".len()..];
    let end = rest.find("</promise>")?;
    let raw = &rest[..end];
    Some(normalize_promise_text(raw))
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

/// 创建状态文件（类似 Claude Code 的 `.claude/ralph-loop.local.md`）
///
/// 格式：
/// ```markdown
/// ---
/// iteration: 1
/// max_iterations: 50
/// completion_promise: COMPLETE
/// started_at: 2026-01-18T10:00:00Z
/// ---
///
/// [原始 prompt]
/// ```
pub(crate) fn create_state_file_content(
    iteration: u32,
    max_iterations: u32,
    completion_promise: &str,
    started_at: &str,
    prompt: &str,
) -> String {
    format!(
        r#"---
active: true
iteration: {iteration}
max_iterations: {max_iterations}
completion_promise: {completion_promise}
started_at: {started_at}
---

{prompt}
"#,
    )
}

/// 保存 Ralph Loop 状态到文件
pub(crate) async fn save_ralph_state_file(
    state: &codex_protocol::protocol::RalphLoopState,
) -> Result<(), std::io::Error> {
    let state_dir = std::path::Path::new(".codex");
    tokio::fs::create_dir_all(state_dir).await?;

    let state_file = state_dir.join("ralph-loop.local.md");
    let content = create_state_file_content(
        state.iteration,
        state.max_iterations,
        &state.completion_promise,
        &state.started_at,
        &state.original_prompt,
    );

    tokio::fs::write(state_file, content).await?;
    Ok(())
}

/// 清理 Ralph Loop 状态文件
pub(crate) async fn cleanup_ralph_state_file() -> Result<(), std::io::Error> {
    let state_file = std::path::Path::new(".codex/ralph-loop.local.md");
    if state_file.exists() {
        tokio::fs::remove_file(state_file).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_check_completion_promise_with_tag() {
        let output = "Some text <promise>COMPLETE</promise> more text";
        assert!(check_completion_promise(output, "COMPLETE"));
    }

    #[test]
    fn test_check_completion_promise_direct_is_not_enough() {
        let output = "Task is COMPLETE";
        assert!(!check_completion_promise(output, "COMPLETE"));
    }

    #[test]
    fn test_check_completion_promise_normalizes_whitespace() {
        let output = "Some text <promise>\n  COMPLETE \t</promise> more text";
        assert!(check_completion_promise(output, "COMPLETE"));
    }

    #[test]
    fn test_check_completion_promise_not_found() {
        let output = "Task is in progress";
        assert!(!check_completion_promise(output, "COMPLETE"));
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("hello", 10), "hello");
        assert_eq!(truncate_string("hello world", 5), "hello...");
    }

    #[test]
    fn test_create_state_file_content() {
        let content =
            create_state_file_content(1, 50, "COMPLETE", "2026-01-18T10:00:00Z", "Build API");

        assert!(content.contains("active: true"));
        assert!(content.contains("iteration: 1"));
        assert!(content.contains("max_iterations: 50"));
        assert!(content.contains("completion_promise: COMPLETE"));
        assert!(content.contains("Build API"));
    }
}
