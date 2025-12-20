use crate::pager_overlay::GitGraphOverlay;
use crate::pager_overlay::Overlay;
use codex_ansi_escape::ansi_escape_line;
use ratatui::style::Stylize;
use ratatui::text::Line;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

// Prefer the embedded git-graph library (round Unicode style) when available.
// This produces rounded connectors (╭╮╯╰) and colored branches. If it fails
// for any reason, we fall back to `git log --graph`.
#[allow(unused_imports)]
use git_graph::config::get_model_name;
#[allow(unused_imports)]
use git_graph::get_repo;
#[allow(unused_imports)]
use git_graph::graph::GitGraph;
#[allow(unused_imports)]
use git_graph::print::format::CommitFormat;
#[allow(unused_imports)]
use git_graph::print::unicode::print_unicode;
#[allow(unused_imports)]
use git_graph::settings::BranchOrder;
#[allow(unused_imports)]
use git_graph::settings::BranchSettings;
#[allow(unused_imports)]
use git_graph::settings::BranchSettingsDef;
#[allow(unused_imports)]
use git_graph::settings::Characters;
#[allow(unused_imports)]
use git_graph::settings::MergePatterns;
#[allow(unused_imports)]
use git_graph::settings::Settings;

/// Convert ASCII art to round/Unicode style.
fn convert_to_round_style(line: &str) -> String {
    let mut result = line.to_string();

    // Replace ASCII graph characters with round Unicode equivalents
    result = result.replace('*', "●"); // Replace asterisk with bullet
    result = result.replace('|', "│"); // Replace pipe with box drawing
    result = result.replace('\\', "╲"); // Replace backslash with diagonal
    result = result.replace('/', "╱"); // Replace forward slash with diagonal
    result = result.replace('-', "─"); // Replace dash with horizontal line

    result
}

/// Generate git graph lines for display in the TUI overlay.
pub(crate) fn generate_git_graph<P: AsRef<Path>>(
    repo_path: P,
) -> Result<Vec<Line<'static>>, String> {
    // First try: high-quality round Unicode graph via git-graph library.
    if let Ok(lines) = generate_with_git_graph(repo_path.as_ref()) {
        return Ok(lines);
    }

    // Fallback: use `git log --graph` (ASCII) and do a best-effort conversion
    // to Unicode line drawing characters.
    let output = Command::new("git")
        .args([
            "log",
            "--graph",
            // Dim the commit hash and the author to visually de-emphasize them
            // relative to the commit subject. We use ANSI SGR for dim (2/22)
            // so it plays nicely with whatever colors git chooses.
            "--pretty=format:%C(auto)\u{1b}[2m%h\u{1b}[22m %s %C(green)(%cr) \u{1b}[2m<%an>\u{1b}[22m%C(auto)%d",
            "--all",
            "--color=always",
            "--abbrev-commit",
        ])
        .current_dir(&repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git log: {e}"))?;

    if !output.status.success() {
        let fallback_output = Command::new("git")
            .args(["log", "--graph", "--oneline", "--all", "--color=always"])
            .current_dir(&repo_path)
            .output()
            .map_err(|e| format!("Failed to execute fallback git log: {e}"))?;

        if !fallback_output.status.success() {
            return Err(format!(
                "Git command failed: {}",
                String::from_utf8_lossy(&fallback_output.stderr)
            ));
        }

        let output_str = String::from_utf8_lossy(&fallback_output.stdout);
        return if output_str.trim().is_empty() {
            Ok(vec!["No git history found.".dim().into()])
        } else {
            let lines: Vec<Line<'static>> = output_str
                .lines()
                .map(|line| ansi_escape_line(&convert_to_round_style(line)))
                .collect();
            Ok(lines)
        };
    }

    let output_str = String::from_utf8_lossy(&output.stdout);

    if output_str.trim().is_empty() {
        Ok(vec!["No git history found.".dim().into()])
    } else {
        let lines: Vec<Line<'static>> = output_str
            .lines()
            .map(|line| ansi_escape_line(&convert_to_round_style(line)))
            .collect();
        Ok(lines)
    }
}

/// Create a new git graph overlay for the TUI.
pub(crate) fn create_git_graph_overlay<P: AsRef<Path>>(repo_path: P) -> Result<Overlay, String> {
    let path = repo_path.as_ref().to_path_buf();
    let lines = generate_git_graph(&path)?;

    let refresh_path = path.clone();
    let refresh_callback: Arc<
        dyn Fn() -> std::result::Result<Vec<Line<'static>>, String> + Send + Sync,
    > = Arc::new(move || generate_git_graph(&refresh_path));

    Ok(Overlay::GitGraph(GitGraphOverlay::new_with_refresh(
        lines,
        "G I T   G R A P H".to_string(),
        refresh_callback,
    )))
}

// Build lines using the embedded git-graph library with a "round" style.
fn generate_with_git_graph<P: AsRef<Path>>(repo_path: P) -> Result<Vec<Line<'static>>, String> {
    let repo = get_repo(repo_path, true).map_err(|e| format!("libgit2 error: {}", e.message()))?;

    let model_name = get_model_name(&repo, "git-graph.toml").unwrap_or(None);
    let model_def = match model_name.as_deref() {
        Some("git-flow") => BranchSettingsDef::git_flow(),
        Some("simple") => BranchSettingsDef::simple(),
        Some("none") => BranchSettingsDef::none(),
        _ => BranchSettingsDef::simple(),
    };
    let branches = BranchSettings::from(model_def).map_err(|e| format!("settings error: {e}"))?;

    let settings = Settings {
        reverse_commit_order: false,
        debug: false,
        compact: true,
        colored: true,
        include_remote: true,
        format: CommitFormat::Short,
        wrapping: None,
        characters: Characters::round(),
        branch_order: BranchOrder::ShortestFirst(true),
        branches,
        merge_patterns: MergePatterns::default(),
    };

    let graph = GitGraph::new(repo, &settings, None)?;
    let (g_lines, t_lines, _indices) = print_unicode(&graph, &settings)?;

    let mut out = Vec::new();
    for (g, t) in g_lines.iter().zip(t_lines.iter()) {
        let combined = format!("{g}{t}");
        out.push(ansi_escape_line(&combined));
    }
    Ok(out)
}
