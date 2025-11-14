//! Agent executor that runs codex with resume-clone in isolated worktrees

use crate::AgentConfig;
use crate::AgentResult;
use crate::SessionRecorder;
use crate::worktree::AgentWorktree;
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

fn load_agent_prompt_template() -> Result<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let default_path = PathBuf::from(home).join(".codex/tumix/tumix-agent.md");
    let prompt_path = std::env::var("TUMIX_AGENT_PROMPT_PATH")
        .map(PathBuf::from)
        .unwrap_or(default_path);

    std::fs::read_to_string(&prompt_path).with_context(|| {
        format!(
            "无法读取 TUMIX agent 提示词，缺少文件：{}。请先创建模板。",
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

/// Executes agents with resume-clone
#[derive(Clone)]
pub struct AgentExecutor {
    parent_session: String,
}

impl AgentExecutor {
    /// Create a new agent executor
    pub fn new(parent_session: String) -> Self {
        Self { parent_session }
    }

    /// Execute a single agent in its worktree
    pub(crate) async fn execute(
        &self,
        config: &AgentConfig,
        worktree: &AgentWorktree,
        session_recorder: Arc<SessionRecorder>,
        run_id: &str,
        cancel_token: CancellationToken,
    ) -> Result<AgentResult> {
        let docs_dir = worktree.path.join(".tumix").join("docs").join(run_id);
        std::fs::create_dir_all(&docs_dir)
            .context("Failed to prepare .tumix/docs directory for agent artifacts")?;

        let run_slug = run_id.replace('-', "_");
        let base_filename = format!("tumix_{run_slug}_agent_{}", config.id);
        let tex_path = docs_dir.join(format!("{base_filename}.tex"));
        let pdf_path = docs_dir.join(format!("{base_filename}.pdf"));

        let tex_abs = match tex_path.canonicalize() {
            Ok(path) => path,
            Err(_) => tex_path.clone(),
        };
        let pdf_abs = match pdf_path.canonicalize() {
            Ok(path) => path,
            Err(_) => pdf_path.clone(),
        };
        let docs_dir_abs = match docs_dir.canonicalize() {
            Ok(path) => path,
            Err(_) => docs_dir.clone(),
        };

        let tex_path_str = tex_abs.to_string_lossy().into_owned();
        let pdf_path_str = pdf_abs.to_string_lossy().into_owned();
        let docs_dir_str = docs_dir_abs.to_string_lossy().into_owned();

        // 1. Build prompt
        let template = load_agent_prompt_template()?;
        let template_body = strip_front_matter(&template);
        let prompt = template_body
            .replace("$NAME", &config.name)
            .replace("$ROLE", &config.role)
            .replace("$TEX_FILE", &tex_path_str)
            .replace("$PDF_FILE", &pdf_path_str)
            .replace("$DOCS_DIR", &docs_dir_str);

        // 2. Execute codex with resume-clone
        let codex_bin = std::env::var("CODEX_BIN").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{}/.npm-global/bin/codex", home)
        });

        // Create a temporary file for session metadata output (use absolute path)
        let project_root = std::env::current_dir().context("Failed to get current directory")?;
        let id_output_path = project_root.join(format!(
            ".tumix/agent-{}-{}-session.json",
            run_id, config.id
        ));
        let id_output_arg = format!("--id-output={}", id_output_path.display());

        // Build command args with --id-output for immediate session info
        let args = vec![
            "exec",
            "--print-rollout-path",
            "--skip-git-repo-check",
            &id_output_arg,
            "--sandbox",
            "danger-full-access",
            "--model",
            "gpt-5-codex-high",
            "resume-clone",
            &self.parent_session,
        ];

        tracing::debug!(
            "Agent {}: Executing command: {} {}",
            config.id,
            codex_bin,
            args.join(" ")
        );

        let mut command = Command::new(&codex_bin);
        command
            .args(args)
            .arg(&prompt)
            .current_dir(&worktree.path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .context(format!("Failed to execute agent {}", config.id))?;

        let stdout_handle = spawn_reader(child.stdout.take());
        let stderr_handle = spawn_reader(child.stderr.take());

        let status = tokio::select! {
            res = child.wait() => res.context("Failed to await codex exec status")?,
            _ = cancel_token.cancelled() => {
                tracing::info!("Cancelling agent {} run {}", config.id, run_id);
                let _ = child.start_kill();
                child
                    .wait()
                    .await
                    .context("Failed to await cancelled codex exec status")?
            }
        };

        let stdout = join_reader(stdout_handle).await;
        let stderr = join_reader(stderr_handle).await;

        if cancel_token.is_cancelled() {
            tracing::warn!("Agent {} execution cancelled", config.id);
            return Err(anyhow!("Agent {} execution cancelled", config.id));
        }

        if !status.success() {
            tracing::error!("Agent {} codex execution failed", config.id);
            tracing::error!("  Exit code: {:?}", status.code());
            if let Some(err) = stderr.as_deref() {
                tracing::error!("  Stderr: {}", err.chars().take(500).collect::<String>());
            }
            if let Some(out) = stdout.as_deref() {
                tracing::error!("  Stdout: {}", out.chars().take(200).collect::<String>());
            }
            anyhow::bail!(
                "Agent {} execution failed with exit code {:?}:
{}",
                config.id,
                status.code(),
                stderr
                    .unwrap_or_else(|| "execution cancelled or failed".to_string())
                    .chars()
                    .take(300)
                    .collect::<String>(),
            );
        }

        tracing::debug!("Agent {}: Command completed successfully", config.id);

        // 3. Read session metadata from the output file written by codex
        let metadata_content = std::fs::read_to_string(&id_output_path).context(format!(
            "Failed to read session metadata from {}",
            id_output_path.display()
        ))?;

        let metadata: serde_json::Value = serde_json::from_str(&metadata_content)
            .context("Failed to parse session metadata JSON")?;

        let session_id = metadata["session_id"]
            .as_str()
            .context("Missing session_id in metadata")?
            .to_string();

        let jsonl_path = metadata["rollout_path"]
            .as_str()
            .context("Missing rollout_path in metadata")?
            .to_string();

        tracing::debug!(
            "Agent {}: New session {} (JSONL: {})",
            config.id,
            &session_id[..8],
            &jsonl_path
        );

        // Immediately update round1_sessions.json with session info
        session_recorder.record_session_start(&config.id, &session_id, &jsonl_path)?;

        // Clean up temporary metadata file
        let _ = std::fs::remove_file(&id_output_path);

        // 4. Auto-commit changes
        let commit_hash = worktree
            .auto_commit()
            .context("Failed to commit agent work")?;

        Ok(AgentResult {
            agent_id: config.id.clone(),
            session_id,
            commit_hash,
            branch: worktree.branch.clone(),
            jsonl_path,
        })
    }
}

fn spawn_reader<R>(reader: Option<R>) -> Option<JoinHandle<anyhow::Result<String>>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    reader.map(|mut pipe| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            pipe.read_to_end(&mut buf)
                .await
                .context("Failed to read process output")?;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        })
    })
}

async fn join_reader(handle: Option<JoinHandle<anyhow::Result<String>>>) -> Option<String> {
    match handle {
        Some(task) => match task.await {
            Ok(Ok(data)) => Some(data),
            Ok(Err(e)) => {
                tracing::error!("Failed to read process output: {e:#}");
                None
            }
            Err(e) => {
                tracing::error!("Reader task panicked: {e:#}");
                None
            }
        },
        None => None,
    }
}
