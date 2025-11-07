use std::collections::HashSet;
use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;

use crate::cxresume_picker_widget::SessionInfo;
use crate::cxresume_picker_widget::get_cwd_sessions;
use crate::cxresume_picker_widget::load_tumix_status_index;

/// Bottom session bar (similar to tmux)
pub struct SessionBar {
    /// List of sessions in current working directory
    sessions: Vec<SessionInfo>,
    /// Currently selected session index
    selected_index: usize,
    /// Whether the bar has focus
    has_focus: bool,
    /// Session loading state
    loading: bool,
    /// Error message if any
    error: Option<String>,
    /// Current active session ID (if any)
    current_session_id: Option<String>,
}

impl SessionBar {
    pub fn new(_cwd: PathBuf) -> Self {
        let mut bar = Self {
            sessions: Vec::new(),
            selected_index: 0,
            has_focus: false,
            loading: false,
            error: None,
            current_session_id: None,
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
        self.sessions.get(self.selected_index)
    }

    /// Move selection left
    pub fn select_previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection right
    pub fn select_next(&mut self) {
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
        self.current_session_id = session_id;
    }

    /// Build the session bar line (similar to tmux status bar)
    ///
    /// Label format: [n]:<short-id> (no "历史/当前" words). The current session is
    /// highlighted by style only. A standalone "[0]:新建" is shown only when the
    /// current session is not present in history.
    fn build_bar_line(&self, current_session_id: Option<&str>) -> Line<'static> {
        if let Some(error) = &self.error {
            return Line::from(vec![
                Span::styled(
                    " Error: ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(error.clone(), Style::default().fg(Color::Red)),
            ]);
        }

        let mut spans = Vec::new();

        // Determine whether the current session exists in history
        let current_in_history = current_session_id
            .map(|id| self.sessions.iter().any(|s| s.id == id))
            .unwrap_or(false);

        // Only show a standalone "新建" when the current session is not in history
        if !current_in_history {
            let new_style = if self.has_focus {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled("[0]", new_style));
            spans.push(Span::styled(":新建", new_style));
        }

        if self.sessions.is_empty() {
            // Only show current session
            spans.push(Span::from(" "));
            spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
            spans.push(Span::from(" "));
            spans.push(Span::styled(
                "No history",
                Style::default().fg(Color::Yellow),
            ));
        } else {
            // Session indicators (like tmux tabs)
            for (idx, session) in self.sessions.iter().enumerate() {
                let display_idx = idx + 1; // Only for display
                let is_selected = self.selected_index == idx; // selection uses real index

                // Tab separator
                spans.push(Span::from(" "));

                // Short id
                let session_id = if session.id.len() > 8 {
                    format!("{}…", &session.id[..7])
                } else {
                    session.id.clone()
                };

                // Check if this is the current active session (style only)
                let is_current = current_session_id.map_or(false, |id| id == session.id);

                let style = if is_selected {
                    if self.has_focus {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Gray)
                            .add_modifier(Modifier::BOLD)
                    }
                } else if is_current {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };

                spans.push(Span::styled(format!("[{}]", display_idx), style));
                spans.push(Span::styled(format!(":{}", session_id), style));

                // Add status indicators
                if session.tumix.is_some() {
                    spans.push(Span::styled(
                        "*",
                        Style::default().fg(Color::Rgb(191, 90, 242)),
                    ));
                }

                // Show message count for non-selected sessions
                if !is_selected && session.message_count > 0 {
                    spans.push(Span::styled(
                        format!("({})", session.message_count),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
            }
        }

        // Right side: status and help
        spans.push(Span::from(" "));
        spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        spans.push(Span::from(" "));
        spans.push(Span::styled("状态:", Style::default().fg(Color::Gray)));
        spans.push(Span::from(" "));

        let (status_label, status_name) = if let Some(cur_id) = current_session_id {
            // current in history or not — label is always "当前"
            let short_cur = if cur_id.len() > 8 {
                format!("{}…", &cur_id[..7])
            } else {
                cur_id.to_string()
            };
            ("当前", short_cur)
        } else {
            ("当前", "新建".to_string())
        };

        spans.push(Span::styled(
            status_label,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::from("  "));
        spans.push(Span::styled("会话:", Style::default().fg(Color::Gray)));
        spans.push(Span::from(" "));
        spans.push(Span::styled(status_name, Style::default()));

        // Help
        spans.push(Span::from(" "));
        spans.push(Span::styled("│", Style::default().fg(Color::DarkGray)));
        spans.push(Span::from(" "));
        if self.has_focus {
            spans.push(Span::styled(
                "←/→",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::from(" Navigate "));
            spans.push(Span::styled(
                "Enter",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::from(" Open "));
            spans.push(Span::styled(
                "Tab",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::from(" Exit"));
        } else {
            spans.push(Span::styled("F1", Style::default().fg(Color::Gray)));
            spans.push(Span::from(" Toggle Bar "));
            spans.push(Span::styled("Ctrl+P", Style::default().fg(Color::Gray)));
            spans.push(Span::from(" Sessions"));
        }

        Line::from(spans)
    }
}

impl WidgetRef for &SessionBar {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Draw a top border line to separate from chat area
        let border_style = Style::default().fg(Color::Rgb(60, 60, 60));
        for x in area.left()..area.right() {
            buf[(x, area.top())].set_symbol("─").set_style(border_style);
        }

        // Adjust area to exclude the border line
        let bar_area = Rect {
            x: area.x,
            y: area.y.saturating_add(1),
            width: area.width,
            height: area.height.saturating_sub(1),
        };

        // Build the status bar line
        let line = self.build_bar_line(self.current_session_id.as_deref());

        // Render with background color and padding
        let style = if self.has_focus {
            Style::default().bg(Color::Rgb(30, 30, 30))
        } else {
            Style::default().bg(Color::Rgb(20, 20, 20))
        };

        // Clear the bar area with background color
        for y in bar_area.top()..bar_area.bottom() {
            for x in bar_area.left()..bar_area.right() {
                buf[(x, y)].set_style(style);
            }
        }

        // Render the session bar with vertical centering
        if bar_area.height > 0 {
            let centered_y = bar_area.y + (bar_area.height.saturating_sub(1) / 2);
            let render_area = Rect {
                x: bar_area.x,
                y: centered_y,
                width: bar_area.width,
                height: 1,
            };
            Paragraph::new(vec![line])
                .style(style)
                .render(render_area, buf);
        }
    }
}
