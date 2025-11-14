use crate::pager_overlay::Overlay;
use codex_ansi_escape::ansi_escape_line;
use ratatui::style::Stylize;
use ratatui::text::Line;
use std::path::Path;
use std::process::Command;

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

/// Convert ASCII art to round/Unicode style
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
pub fn generate_git_graph<P: AsRef<Path>>(repo_path: P) -> Result<Vec<Line<'static>>, String> {
    // First try: high-quality round Unicode graph via git-graph library.
    if let Ok(lines) = generate_with_git_graph(repo_path.as_ref()) {
        return Ok(lines);
    }

    // Fallback: use `git log --graph` (ASCII) and do a best-effort conversion
    // to Unicode line drawing characters.
    // Try git log with detailed formatting and more commits for scrolling
    // Show the full history (no artificial commit limit); the pager overlay
    // handles scrolling efficiently. This avoids surprising truncation in
    // larger repos where 50 commits isn't enough.
    let output = Command::new("git")
        .args([
            "log",
            "--graph",
            // Dim the commit hash and the author to visually de-emphasize them
            // relative to the commit subject. We use ANSI SGR for dim (2/22)
            // so it plays nicely with whatever colors git chooses.
            "--pretty=format:%C(auto)\x1b[2m%h\x1b[22m %s %C(green)(%cr) \x1b[2m<%an>\x1b[22m%C(auto)%d",
            "--all",
            "--color=always",
            "--abbrev-commit",
        ])
        .current_dir(&repo_path)
        .output()
        .map_err(|e| format!("Failed to execute git log: {e}"))?;

    if !output.status.success() {
        // Fallback to simpler git log if the above fails
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
            // Convert to round style and process ANSI
            let lines: Vec<Line<'static>> = output_str
                .lines()
                .map(|line| {
                    let round_line = convert_to_round_style(line);
                    ansi_escape_line(&round_line)
                })
                .collect();
            Ok(lines)
        };
    }

    let output_str = String::from_utf8_lossy(&output.stdout);

    if output_str.trim().is_empty() {
        Ok(vec!["No git history found.".dim().into()])
    } else {
        // Convert each line to round style, then process ANSI escapes
        let lines: Vec<Line<'static>> = output_str
            .lines()
            .map(|line| {
                let round_line = convert_to_round_style(line);
                ansi_escape_line(&round_line)
            })
            .collect();
        Ok(lines)
    }
}

/// Create a new git graph overlay for the TUI with enhanced title.
pub fn create_git_graph_overlay<P: AsRef<Path>>(repo_path: P) -> Result<Overlay, String> {
    let path = repo_path.as_ref().to_path_buf();
    let lines = generate_git_graph(&path)?;

    // Create a refresh callback that regenerates the git graph
    let refresh_callback = Box::new(move || generate_git_graph(&path));

    Ok(Overlay::new_static_with_title_no_wrap_refresh(
        lines,
        "G I T   G R A P H   │   j/k:scroll   r:refresh   q/Esc:close   │   C t r l + G"
            .to_string(),
        refresh_callback,
    ))
}

// Build lines using the embedded git-graph library with a "round" style.
fn generate_with_git_graph<P: AsRef<Path>>(repo_path: P) -> Result<Vec<Line<'static>>, String> {
    // Discover the repository; allow owner validation to be skipped to avoid
    // platform-specific errors in embedded environments.
    let repo = get_repo(repo_path, true).map_err(|e| format!("libgit2 error: {}", e.message()))?;

    // Load model preference if present; otherwise use a reasonable default.
    let model_name = get_model_name(&repo, "git-graph.toml").unwrap_or(None);
    let model_def = match model_name.as_deref() {
        Some("git-flow") => BranchSettingsDef::git_flow(),
        Some("simple") => BranchSettingsDef::simple(),
        Some("none") => BranchSettingsDef::none(),
        _ => BranchSettingsDef::simple(),
    };
    let branches = BranchSettings::from(model_def).map_err(|e| format!("settings error: {e}"))?;

    // Use rounded characters, include remotes, and colored output like `--all --color`.
    let settings = Settings {
        reverse_commit_order: false,
        debug: false,
        compact: true,
        colored: true,
        include_remote: true,
        // Compact commit summary similar to `--oneline`.
        format: CommitFormat::Short,
        // Let our TUI pager handle wrapping.
        wrapping: None,
        characters: Characters::round(),
        branch_order: BranchOrder::ShortestFirst(true),
        branches,
        merge_patterns: MergePatterns::default(),
    };

    // No artificial limit: let the pager scroll the full history.
    let graph = GitGraph::new(repo, &settings, None)?;
    let (g_lines, t_lines, _indices) = print_unicode(&graph, &settings)?;

    // Join graph and text columns, then parse ANSI into ratatui Line.
    // For the embedded printer path, post-process the text column to dim the
    // commit hash (first token) and the author section (e.g., "<name>") so
    // the commit subject stands out.
    #[allow(dead_code)]
    fn dim_hash_and_author(s: &str) -> String {
        // Insert dim (ESC[2m) at the first visible character (skipping any
        // leading ANSI), and end dim (ESC[22m) before the first following
        // visible whitespace. Then, if present, also wrap the first
        // angle-bracketed author section with dim toggles.
        let bytes = s.as_bytes();
        let mut i = 0;
        let len = bytes.len();
        // Helper to skip a CSI sequence starting at ESC
        let skip_ansi = |idx: &mut usize| {
            if *idx < len && bytes[*idx] == 0x1b {
                *idx += 1;
                if *idx < len && bytes[*idx] == b'[' {
                    *idx += 1;
                    while *idx < len && bytes[*idx] != b'm' {
                        *idx += 1;
                    }
                    if *idx < len {
                        *idx += 1;
                    }
                }
            }
        };

        // Find first visible, non-space character using char boundaries
        let mut char_iter = s.char_indices();
        let mut start_idx = 0;

        while i < len {
            if bytes[i] == 0x1b {
                skip_ansi(&mut i);
                continue;
            }
            if let Some((idx, ch)) = char_iter.next() {
                i = idx + ch.len_utf8();
                if !ch.is_whitespace() {
                    start_idx = idx;
                    break;
                }
            } else {
                break;
            }
        }

        // If nothing visible, return original
        if start_idx >= s.len() {
            return s.to_string();
        }

        // Find end of the first token using char boundaries
        let mut end_idx = start_idx;
        for (idx, ch) in s[start_idx..].char_indices() {
            if ch.is_whitespace() {
                end_idx = start_idx + idx;
                break;
            }
            end_idx = start_idx + idx + ch.len_utf8();
        }

        // Ensure we don't have ANSI codes in our indices
        // Skip if start or end is in middle of ANSI sequence
        if s.as_bytes().get(start_idx) == Some(&0x1b) || s.as_bytes().get(end_idx) == Some(&0x1b) {
            return s.to_string();
        }

        let start = start_idx;
        let end = end_idx;

        // Compose with dim around [start, end)
        let mut out = String::with_capacity(s.len() + 16);
        out.push_str(&s[..start]);
        out.push_str("\x1b[2m");
        out.push_str(&s[start..end]);
        out.push_str("\x1b[22m");
        out.push_str(&s[end..]);

        // Now dim the first "<...>" block if present.
        if let Some(a_start) = out.find('<')
            && let Some(a_end_rel) = out[a_start..].find('>')
        {
            let a_end = a_start + a_end_rel;
            if a_end > a_start {
                let mut final_out = String::with_capacity(out.len() + 16);
                final_out.push_str(&out[..a_start]);
                final_out.push_str("\x1b[2m");
                final_out.push_str(&out[a_start..=a_end]);
                final_out.push_str("\x1b[22m");
                final_out.push_str(&out[a_end + 1..]);
                return final_out;
            }
        }

        out
    }

    let lines: Vec<Line<'static>> = g_lines
        .into_iter()
        .zip(t_lines)
        .map(|(g, t)| {
            // Skip dim_hash_and_author for non-ASCII commits (e.g., Chinese)
            // to avoid UTF-8 boundary issues
            ansi_escape_line(&format!(" {g}  {t}"))
        })
        .collect();
    Ok(lines)
}
