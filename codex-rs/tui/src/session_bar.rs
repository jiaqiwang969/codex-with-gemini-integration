use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize; // Prefer Stylize helpers for consistent styling
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;
use unicode_width::UnicodeWidthStr;

use crate::cxresume_picker_widget::SessionInfo;
use crate::cxresume_picker_widget::TumixState;
use crate::cxresume_picker_widget::get_cwd_sessions;
use crate::cxresume_picker_widget::last_user_snippet;
use crate::cxresume_picker_widget::load_tumix_status_index;
use crate::key_hint; // Unify key-hint rendering
use crate::session_alias_manager::SessionAliasManager;
use crossterm::event::KeyCode;

/// Bottom session bar (similar to tmux)
pub struct SessionBar {
    /// List of sessions in current working directory
    sessions: Vec<SessionInfo>,
    /// Currently selected session index
    selected_index: usize,
    /// Whether selection is on the special "新建" tab
    selected_on_new: bool,
    /// Whether the bar has focus
    has_focus: bool,
    /// Session loading state
    loading: bool,
    /// Error message if any
    error: Option<String>,
    /// Current active session ID (if any)
    current_session_id: Option<String>,
    /// Status for the current session only
    current_session_status: Option<String>,
    /// Cached labels derived from first user message (by path)
    label_cache: HashMap<PathBuf, String>,
    /// Session alias manager
    alias_manager: SessionAliasManager,
}

impl SessionBar {
    pub fn new(_cwd: PathBuf) -> Self {
        let mut bar = Self {
            sessions: Vec::new(),
            selected_index: 0,
            selected_on_new: false,
            has_focus: false,
            loading: false,
            error: None,
            current_session_id: None,
            current_session_status: None,
            label_cache: HashMap::new(),
            alias_manager: SessionAliasManager::load(),
        };

        // Load sessions on creation
        bar.refresh_sessions();
        bar
    }

    /// Refresh the session list from disk
    pub fn refresh_sessions(&mut self) {
        self.loading = true;
        self.error = None;

        match get_cwd_sessions() {
            Ok(mut sessions) => {
                // Add tumix status if available
                let tumix_index = load_tumix_status_index();
                for session in &mut sessions {
                    if let Some(indicator) = tumix_index.lookup(&session.id, &session.path) {
                        session.tumix = Some(indicator);
                    }
                }

                // Sort is already mtime desc in provider; de-duplicate by id (keep newest)
                let mut seen = HashSet::new();
                sessions.retain(|s| seen.insert(s.id.clone()));

                self.sessions = sessions;
                self.loading = false;

                // If current session is in history, select it by default
                if let Some(cur) = self.current_session_id.as_ref() {
                    if let Some(pos) = self.sessions.iter().position(|s| &s.id == cur) {
                        self.selected_index = pos;
                    }
                }

                // Keep selection in bounds
                if self.selected_index >= self.sessions.len() && !self.sessions.is_empty() {
                    self.selected_index = self.sessions.len() - 1;
                }

                // Compute labels lazily for visible sessions (cache by path)
                for s in &self.sessions {
                    // Always recompute for the current session so alias follows latest user message
                    let must_update = self
                        .current_session_id
                        .as_ref()
                        .map(|id| *id == s.id)
                        .unwrap_or(false)
                        || !self.label_cache.contains_key(&s.path);
                    if must_update {
                        if let Some(snippet) = last_user_snippet(&s.path, 5) {
                            // Unicode-safe truncation to keep bar compact
                            let short = if snippet.chars().count() > 10 {
                                let truncated: String = snippet.chars().take(10).collect();
                                format!("{}…", truncated)
                            } else {
                                snippet
                            };
                            self.label_cache.insert(s.path.clone(), short);
                        }
                    }
                }
            }
            Err(e) => {
                self.error = Some(e);
                self.loading = false;
                self.sessions.clear();
            }
        }
    }

    /// Get the currently selected session
    pub fn selected_session(&self) -> Option<&SessionInfo> {
        if self.selected_on_new {
            None
        } else {
            self.sessions.get(self.selected_index)
        }
    }

    /// Is the special "新建" tab currently selected
    pub fn selected_is_new(&self) -> bool {
        self.selected_on_new
    }

    /// Move selection left
    pub fn select_previous(&mut self) {
        if self.selected_on_new {
            // already at the left-most before first session
            return;
        }
        if self.selected_index > 0 {
            self.selected_index -= 1;
        } else {
            // At first session; if a New tab exists, move to it
            let has_new = self
                .current_session_id
                .as_deref()
                .map(|id| !self.sessions.iter().any(|s| s.id == id))
                .unwrap_or(true);
            if has_new {
                self.selected_on_new = true;
            }
        }
    }

    /// Move selection right
    pub fn select_next(&mut self) {
        if self.selected_on_new {
            // Leave the New tab and go to the first session if any
            self.selected_on_new = false;
            if !self.sessions.is_empty() {
                self.selected_index = 0;
            }
            return;
        }
        if self.selected_index < self.sessions.len().saturating_sub(1) {
            self.selected_index += 1;
        }
    }

    /// Set focus state
    pub fn set_focus(&mut self, focused: bool) {
        self.has_focus = focused;
    }

    /// Get focus state
    pub fn has_focus(&self) -> bool {
        self.has_focus
    }

    /// Set current session ID
    pub fn set_current_session(&mut self, session_id: Option<String>) {
        // Clear status when switching sessions
        if self.current_session_id != session_id {
            self.current_session_status = None;
        }
        self.current_session_id = session_id;
    }

    /// Update status text for the current session only
    pub fn set_session_status(&mut self, session_id: String, status: String) {
        // Only update if it's the current session
        if self.current_session_id.as_ref() == Some(&session_id) {
            self.current_session_status = Some(status);
        }
    }

    /// Reset selection when the bar gains focus: select current if present, else select "新建".
    pub fn reset_selection_for_focus(&mut self, current_session_id: Option<&str>) {
        if let Some(id) = current_session_id {
            if let Some(pos) = self.sessions.iter().position(|s| s.id == id) {
                self.selected_index = pos;
                self.selected_on_new = false;
                return;
            }
        }
        // Current not in history -> select New if visible
        self.selected_on_new = true;
        if !self.sessions.is_empty() {
            self.selected_index = 0;
        }
    }

    /// Set alias for a session
    pub fn set_session_alias(&mut self, session_id: String, alias: String) {
        self.alias_manager.set_alias(session_id, alias);
    }

    /// Remove alias for a session (e.g., when deleting a session)
    pub fn remove_session_alias(&mut self, session_id: &str) {
        self.alias_manager.remove_alias(session_id);
    }

    /// Build the session bar lines (similar to tmux status bar)
    ///
    /// Label format: Alias/ShortID (no numbering). The current session is
    /// highlighted by style only. A standalone "新建" is shown only when the
    /// current session is not present in history.
    /// 
    /// Returns: (sessions_line, status_line, help_line, sel_start, sel_end, total_left_width)
    fn build_bar_lines(
        &self,
        current_session_id: Option<&str>,
    ) -> (Line<'static>, Line<'static>, Line<'static>, Option<u16>, Option<u16>, u16) {
        if let Some(error) = &self.error {
            return (
                Line::from(vec![
                    Span::from(" Error: ").red().bold(),
                    Span::from(error.clone()).red(),
                ]),
                Line::from(""),
                Line::from(""),
                None,
                None,
                0,
            );
        }

        let mut left_spans = Vec::new();
        let mut cur_x: u16 = 0;
        let mut sel_start: Option<u16> = None;
        let mut sel_end: Option<u16> = None;
        let add_left =
            |spans: &mut Vec<Span<'static>>, cur_x: &mut u16, text: String, style: Style| {
                *cur_x = cur_x.saturating_add(UnicodeWidthStr::width(text.as_str()) as u16);
                spans.push(Span::styled(text, style));
            };

        // Determine whether the current session exists in history
        let current_in_history = current_session_id
            .map(|id| self.sessions.iter().any(|s| s.id == id))
            .unwrap_or(false);

        // Only show a standalone "新建" when the current session is not in history
        if !current_in_history {
            // Focused + selected → cyan + bold; otherwise dim to let theme drive appearance.
            let new_style = if self.has_focus && self.selected_on_new {
                Style::default().cyan().add_modifier(Modifier::BOLD)
            } else {
                Style::default().dim()
            };
            if self.has_focus && self.selected_on_new && sel_start.is_none() {
                sel_start = Some(cur_x);
            }
            add_left(&mut left_spans, &mut cur_x, "新建".to_string(), new_style);
            if self.has_focus && self.selected_on_new {
                sel_end = Some(cur_x);
            }
        }

        if self.sessions.is_empty() {
            add_left(
                &mut left_spans,
                &mut cur_x,
                " ".to_string(),
                Style::default(),
            );
            left_spans.push(Span::from("│").dim());
            add_left(
                &mut left_spans,
                &mut cur_x,
                " ".to_string(),
                Style::default(),
            );
            left_spans.push(Span::from("No history").italic().dim());
        } else {
            for (idx, session) in self.sessions.iter().enumerate() {
                let is_selected = self.selected_index == idx;
                
                // Add separator before each session except the first
                if idx > 0 || !current_in_history {
                    add_left(
                        &mut left_spans,
                        &mut cur_x,
                        " • ".to_string(),
                        Style::default().dim(),
                    );
                } else {
                    add_left(
                        &mut left_spans,
                        &mut cur_x,
                        " ".to_string(),
                        Style::default(),
                    );
                }

                let session_id = if session.id.len() > 8 {
                    format!("{}…", &session.id[..7])
                } else {
                    session.id.clone()
                };

                let is_current = current_session_id.map_or(false, |id| id == session.id);
                // Selection/current styling aligned with Codex conventions:
                // - Focused + selected: cyan + bold
                // - Current session (regardless of focus): green + bold
                // - Focused + selected (non-current): cyan + bold
                // - Otherwise: default
                let style = if is_current {
                    // 当前会话始终用绿色高亮
                    Style::default().green().add_modifier(Modifier::BOLD)
                } else if self.has_focus && is_selected {
                    // 非当前会话但被选中时用青色
                    Style::default().cyan().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                // Get display name: prefer alias, fall back to snippet or ID
                let display_name = if let Some(alias) = self.alias_manager.get_alias(&session.id) {
                    alias
                } else if let Some(snippet) = self.label_cache.get(&session.path) {
                    if snippet.is_empty() {
                        session_id.clone()
                    } else {
                        format!("{} · {}", snippet, session_id)
                    }
                } else {
                    session_id.clone()
                };

                if is_selected && sel_start.is_none() {
                    sel_start = Some(cur_x);
                }
                add_left(&mut left_spans, &mut cur_x, display_name, style);
                if is_selected {
                    sel_end = Some(cur_x);
                }

                if let Some(ind) = session.tumix.as_ref() {
                    let (label, color) = match ind.state {
                        TumixState::Running => ("运行", Color::Yellow),
                        TumixState::Completed => ("完成", Color::Green),
                        TumixState::Failed => ("失败", Color::Red),
                        TumixState::Stalled => ("停滞", Color::Magenta),
                    };
                    left_spans.push(Span::styled(
                        format!(" · {}", label),
                        Style::default().fg(color),
                    ));
                }
                if !is_selected && session.message_count > 0 {
                    left_spans.push(Span::from(format!("({})", session.message_count)).dim());
                }
            }
        }

        // Build status line (right side of first line)
        let mut status_spans: Vec<Span<'static>> = Vec::new();
        status_spans.push(Span::from(" 状态:").dim());
        status_spans.push(Span::from(" "));
        // Build primary status label and current session short name
        let (status_label, status_name) = if let Some(cur_id) = current_session_id {
            // 优先使用别名，否则使用短ID
            let display_name = if let Some(alias) = self.alias_manager.get_alias(cur_id) {
                alias
            } else {
                if cur_id.len() > 8 {
                    format!("{}…", &cur_id[..7])
                } else {
                    cur_id.to_string()
                }
            };
            let st = self
                .current_session_status
                .clone()
                .unwrap_or_else(|| "就绪".to_string());
            (st, display_name)
        } else {
            ("就绪".to_string(), "新建".to_string())
        };
        status_spans.push(Span::from(status_label).green().bold());
        status_spans.push(Span::from("  "));
        status_spans.push(Span::from("会话:").dim());
        status_spans.push(Span::from(" "));
        status_spans.push(Span::from(status_name).bold());

        // Build help line (second line with keyboard shortcuts)
        let mut help_spans: Vec<Span<'static>> = Vec::new();
        if self.has_focus {
            // Shared key-hint style; all hint texts are dim like the rest of Codex UI
            help_spans.push(Span::from("  "));  // Indent for alignment
            help_spans.push(key_hint::plain(KeyCode::Left).into());
            help_spans.push(Span::from("/".to_string()).dim());
            help_spans.push(key_hint::plain(KeyCode::Right).into());
            help_spans.push(Span::from(" move  ").dim());

            help_spans.push(key_hint::plain(KeyCode::Enter).into());
            help_spans.push(Span::from(" open  ").dim());

            help_spans.push(key_hint::plain(KeyCode::Char('n')).into());
            help_spans.push(Span::from(" new  ").dim());

            help_spans.push(key_hint::plain(KeyCode::Char('r')).into());
            help_spans.push(Span::from(" rename  ").dim());

            help_spans.push(key_hint::plain(KeyCode::Char('x')).into());
            help_spans.push(Span::from(" delete  ").dim());

            // Use Esc to exit session focus; Tab is reserved elsewhere and disabled here
            help_spans.push(key_hint::plain(KeyCode::Esc).into());
            help_spans.push(Span::from(" exit").dim());
        } else {
            help_spans.push(Span::from("  "));  // Indent for alignment
            help_spans.push(key_hint::ctrl(KeyCode::Char('p')).into());
            help_spans.push(Span::from(" Sessions").dim());
        }

        (
            Line::from(left_spans),
            Line::from(status_spans),
            Line::from(help_spans),
            sel_start,
            sel_end,
            cur_x,
        )
    }
}

impl WidgetRef for &SessionBar {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Draw a top border line to separate from chat area (dim, theme-friendly)
        let border_rect = Rect::new(area.x, area.y, area.width, 1);
        Span::from("─".repeat(border_rect.width as usize))
            .dim()
            .render_ref(border_rect, buf);

        // Adjust area to exclude the border line
        let bar_area = Rect {
            x: area.x,
            y: area.y.saturating_add(1),
            width: area.width,
            height: area.height.saturating_sub(1),
        };

        // Build the status bar lines and scrolling metadata
        let (sessions_line, status_line, help_line, sel_start, sel_end, total_left_width) =
            self.build_bar_lines(self.current_session_id.as_deref());

        // Clear the bar area without forcing background colors so terminal themes apply.
        Clear.render(bar_area, buf);

        // Render two lines: sessions/status on first line, help on second line
        if bar_area.height > 0 {
            // First line: sessions list + status
            let first_line_y = bar_area.y;
            let first_line_area = Rect {
                x: bar_area.x,
                y: first_line_y,
                width: bar_area.width,
                height: 1,
            };
            
            // Measure status line width for right side
            let status_width: u16 = status_line
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()) as u16)
                .sum();

            let sessions_width = first_line_area
                .width
                .saturating_sub(status_width.saturating_add(3)); // 3 for separator
            let sessions_area = Rect {
                x: first_line_area.x,
                y: first_line_area.y,
                width: sessions_width,
                height: 1,
            };

            // Draw separator and status on right side
            if status_width > 0 && sessions_width < first_line_area.width {
                let sep_x = first_line_area.x + sessions_width;
                if sep_x < first_line_area.x + first_line_area.width {
                    Span::from(" │ ")
                        .dim()
                        .render_ref(Rect::new(sep_x, first_line_area.y, 3, 1), buf);
                }
                let status_area = Rect {
                    x: first_line_area.x + first_line_area.width.saturating_sub(status_width),
                    y: first_line_area.y,
                    width: status_width,
                    height: 1,
                };
                Paragraph::new(vec![status_line.clone()]).render(status_area, buf);
            }

            // Compute horizontal scroll for sessions list: center selected when possible
            let mut scroll_x: u16 = 0;
            if let (Some(start), Some(end)) = (sel_start, sel_end) {
                let sel_center = start.saturating_add(end).saturating_div(2);
                let half = sessions_area.width.saturating_div(2);
                let desired = sel_center.saturating_sub(half);
                let max_scroll = total_left_width.saturating_sub(sessions_area.width);
                scroll_x = desired.min(max_scroll);
            } else if total_left_width > sessions_area.width {
                scroll_x = total_left_width.saturating_sub(sessions_area.width);
            }

            Paragraph::new(vec![sessions_line])
                .scroll((0, scroll_x))
                .render(sessions_area, buf);

            // Second line: help/keyboard shortcuts
            if bar_area.height > 1 {
                let second_line_y = bar_area.y + 1;
                let second_line_area = Rect {
                    x: bar_area.x,
                    y: second_line_y,
                    width: bar_area.width,
                    height: 1,
                };
                Paragraph::new(vec![help_line]).render(second_line_area, buf);
            }
        }
    }
}
