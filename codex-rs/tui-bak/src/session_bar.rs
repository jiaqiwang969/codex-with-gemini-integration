use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget as _;
use ratatui::widgets::WidgetRef;
use unicode_width::UnicodeWidthStr;

use crate::cxresume_picker_widget::SessionInfo;
use crate::cxresume_picker_widget::get_cwd_sessions;
use crate::cxresume_picker_widget::last_user_snippet;
use crate::cxresume_picker_widget::load_tumix_status_index;
use crate::key_hint;
use crate::session_alias_manager::SessionAliasManager;

/// Bottom session bar (similar to tmux).
pub(crate) struct SessionBar {
    codex_home: PathBuf,
    cwd: PathBuf,

    sessions: Vec<SessionInfo>,
    selected_index: usize,
    selected_on_new: bool,
    has_focus: bool,
    loading: bool,
    error: Option<String>,
    current_session_id: Option<String>,
    current_session_status: Option<String>,
    label_cache: HashMap<PathBuf, String>,
    alias_manager: SessionAliasManager,
}

impl SessionBar {
    pub(crate) fn new(codex_home: PathBuf, cwd: PathBuf) -> Self {
        let mut bar = Self {
            codex_home: codex_home.clone(),
            cwd,
            sessions: Vec::new(),
            selected_index: 0,
            selected_on_new: false,
            has_focus: false,
            loading: false,
            error: None,
            current_session_id: None,
            current_session_status: None,
            label_cache: HashMap::new(),
            alias_manager: SessionAliasManager::load(codex_home),
        };

        bar.refresh_sessions();
        bar
    }

    pub(crate) fn refresh_sessions(&mut self) {
        let preserve_user_selection = self.has_focus;
        let previous_selected_index = self.selected_index;
        let previous_selected_on_new = self.selected_on_new;
        let previous_selected_id = if self.selected_on_new {
            None
        } else {
            self.sessions.get(self.selected_index).map(|s| s.id.clone())
        };

        self.loading = true;
        self.error = None;
        self.alias_manager = SessionAliasManager::load(self.codex_home.clone());

        match get_cwd_sessions(&self.codex_home, &self.cwd) {
            Ok(mut sessions) => {
                let tumix_index = load_tumix_status_index();
                for session in &mut sessions {
                    if let Some(indicator) = tumix_index.lookup(&session.id, &session.path) {
                        session.tumix = Some(indicator);
                    }
                }

                let mut seen = HashSet::new();
                sessions.retain(|s| seen.insert(s.id.clone()));

                self.sessions = sessions;
                self.loading = false;

                self.reconcile_selection_after_refresh(
                    previous_selected_id,
                    previous_selected_index,
                    previous_selected_on_new,
                    preserve_user_selection,
                );

                for s in &self.sessions {
                    let must_update = self
                        .current_session_id
                        .as_ref()
                        .map(|id| *id == s.id)
                        .unwrap_or(false)
                        || !self.label_cache.contains_key(&s.path);
                    if must_update && let Some(snippet) = last_user_snippet(&s.path, 5) {
                        let short = if snippet.chars().count() > 10 {
                            let truncated: String = snippet.chars().take(10).collect();
                            format!("{truncated}…")
                        } else {
                            snippet
                        };
                        self.label_cache.insert(s.path.clone(), short);
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

    fn reconcile_selection_after_refresh(
        &mut self,
        previous_selected_id: Option<String>,
        previous_selected_index: usize,
        previous_selected_on_new: bool,
        preserve_user_selection: bool,
    ) {
        let current_in_history = self
            .current_session_id
            .as_ref()
            .is_some_and(|cur| self.sessions.iter().any(|s| s.id == *cur));

        if current_in_history {
            self.selected_on_new = false;
            if preserve_user_selection
                && previous_selected_on_new
                && let Some(cur) = self.current_session_id.as_ref()
                && let Some(pos) = self.sessions.iter().position(|s| &s.id == cur)
            {
                self.selected_index = pos;
                return;
            }
        } else {
            self.selected_on_new = previous_selected_on_new;
        }

        if preserve_user_selection {
            if self.selected_on_new {
                return;
            }

            if let Some(selected_id) = previous_selected_id
                && let Some(pos) = self.sessions.iter().position(|s| s.id == selected_id)
            {
                self.selected_index = pos;
                return;
            }

            if !self.sessions.is_empty() {
                self.selected_index = previous_selected_index.min(self.sessions.len() - 1);
            } else {
                self.selected_index = 0;
            }
            return;
        }

        if let Some(cur) = self.current_session_id.as_ref()
            && let Some(pos) = self.sessions.iter().position(|s| &s.id == cur)
        {
            self.selected_index = pos;
        }

        if self.selected_index >= self.sessions.len() && !self.sessions.is_empty() {
            self.selected_index = self.sessions.len() - 1;
        }
    }

    pub(crate) fn selected_session(&self) -> Option<&SessionInfo> {
        if self.selected_on_new {
            None
        } else {
            self.sessions.get(self.selected_index)
        }
    }

    pub(crate) fn selected_is_new(&self) -> bool {
        self.selected_on_new
    }

    pub(crate) fn select_previous(&mut self) {
        if self.selected_on_new {
            return;
        }
        if self.selected_index > 0 {
            self.selected_index -= 1;
        } else {
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

    pub(crate) fn select_next(&mut self) {
        if self.selected_on_new {
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

    pub(crate) fn set_focus(&mut self, focused: bool) {
        self.has_focus = focused;
    }

    pub(crate) fn set_current_session(&mut self, session_id: Option<String>) {
        if self.current_session_id != session_id {
            self.current_session_status = None;
        }
        self.current_session_id = session_id;
    }

    pub(crate) fn set_session_status(&mut self, session_id: String, status: String) {
        if self.current_session_id.as_ref() == Some(&session_id) {
            self.current_session_status = Some(status);
        }
    }

    pub(crate) fn reset_selection_for_focus(&mut self, current_session_id: Option<&str>) {
        if let Some(id) = current_session_id
            && let Some(pos) = self.sessions.iter().position(|s| s.id == id)
        {
            self.selected_index = pos;
            self.selected_on_new = false;
            return;
        }
        self.selected_on_new = true;
        if !self.sessions.is_empty() {
            self.selected_index = 0;
        }
    }

    pub(crate) fn set_session_alias(&mut self, session_id: String, alias: String) {
        self.alias_manager.set_alias(session_id, alias);
    }

    pub(crate) fn remove_session_alias(&mut self, session_id: &str) {
        self.alias_manager.remove_alias(session_id);
    }

    fn build_bar_lines(
        &self,
        current_session_id: Option<&str>,
    ) -> (
        Line<'static>,
        Line<'static>,
        Line<'static>,
        Option<u16>,
        Option<u16>,
        u16,
    ) {
        if let Some(error) = &self.error {
            return (
                vec![Span::from(" Error: ").red().bold(), error.clone().red()].into(),
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

        let current_in_history = current_session_id
            .map(|id| self.sessions.iter().any(|s| s.id == id))
            .unwrap_or(false);

        if !current_in_history {
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
            left_spans.push("│".dim());
            add_left(
                &mut left_spans,
                &mut cur_x,
                " ".to_string(),
                Style::default(),
            );
            left_spans.push("No history".italic().dim());
        } else {
            for (idx, session) in self.sessions.iter().enumerate() {
                let is_selected = self.selected_index == idx;

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

                let is_current = current_session_id.is_some_and(|id| id == session.id);
                let style = if is_current {
                    Style::default().green().add_modifier(Modifier::BOLD)
                } else if self.has_focus && is_selected {
                    Style::default().cyan().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                let display_name = if let Some(alias) = self.alias_manager.get_alias(&session.id) {
                    alias
                } else if let Some(snippet) = self.label_cache.get(&session.path) {
                    if snippet.is_empty() {
                        session_id.clone()
                    } else {
                        snippet.clone()
                    }
                } else {
                    session_id.clone()
                };

                if self.has_focus && is_selected && sel_start.is_none() {
                    sel_start = Some(cur_x);
                }
                add_left(&mut left_spans, &mut cur_x, display_name, style);
                if self.has_focus && is_selected {
                    sel_end = Some(cur_x);
                }
            }
        }

        let mut status_spans: Vec<Span<'static>> = Vec::new();
        status_spans.push(" 状态:".dim());
        status_spans.push(" ".into());
        let (status_label, status_name) = if let Some(cur_id) = current_session_id {
            let display_name = if let Some(alias) = self.alias_manager.get_alias(cur_id) {
                alias
            } else if cur_id.len() > 8 {
                format!("{}…", &cur_id[..7])
            } else {
                cur_id.to_string()
            };
            let st = self
                .current_session_status
                .clone()
                .unwrap_or_else(|| "就绪".to_string());
            (st, display_name)
        } else {
            ("就绪".to_string(), "新建".to_string())
        };
        status_spans.push(status_label.green().bold());
        status_spans.push("  ".into());
        status_spans.push("会话:".dim());
        status_spans.push(" ".into());
        status_spans.push(status_name.bold());

        let mut help_spans: Vec<Span<'static>> = Vec::new();
        if self.has_focus {
            help_spans.push(key_hint::plain(KeyCode::Left).into());
            help_spans.push("/".dim());
            help_spans.push(key_hint::plain(KeyCode::Right).into());
            help_spans.push(" move  ".dim());

            help_spans.push(key_hint::plain(KeyCode::Enter).into());
            help_spans.push(" open  ".dim());

            help_spans.push(key_hint::plain(KeyCode::Char('n')).into());
            help_spans.push(" new  ".dim());

            help_spans.push(key_hint::plain(KeyCode::Char('r')).into());
            help_spans.push(" rename  ".dim());

            help_spans.push(key_hint::plain(KeyCode::Char('c')).into());
            help_spans.push(" clone  ".dim());

            help_spans.push(key_hint::plain(KeyCode::Char('x')).into());
            help_spans.push(" delete  ".dim());

            help_spans.push(key_hint::plain(KeyCode::Esc).into());
            help_spans.push(" exit".dim());
        } else {
            help_spans.push(key_hint::ctrl(KeyCode::Char('p')).into());
            help_spans.push(" Sessions".dim());
        }

        (
            left_spans.into(),
            status_spans.into(),
            help_spans.into(),
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

        let border_rect = Rect::new(area.x, area.y, area.width, 1);
        "─"
            .repeat(border_rect.width as usize)
            .dim()
            .render_ref(border_rect, buf);

        let bar_area = Rect {
            x: area.x,
            y: area.y.saturating_add(1),
            width: area.width,
            height: area.height.saturating_sub(1),
        };

        let (sessions_line, status_line, help_line, sel_start, sel_end, total_left_width) =
            self.build_bar_lines(self.current_session_id.as_deref());

        Clear.render(bar_area, buf);

        if bar_area.height > 0 {
            let first_line_area = Rect {
                x: bar_area.x,
                y: bar_area.y,
                width: bar_area.width,
                height: 1,
            };

            let status_width: u16 = status_line
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()) as u16)
                .sum();

            let sessions_width = first_line_area
                .width
                .saturating_sub(status_width.saturating_add(3));
            let sessions_area = Rect {
                x: first_line_area.x,
                y: first_line_area.y,
                width: sessions_width,
                height: 1,
            };

            if status_width > 0 && sessions_width < first_line_area.width {
                let sep_x = first_line_area.x + sessions_width;
                if sep_x < first_line_area.x + first_line_area.width {
                    " │ "
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

            let mut scroll_x: u16 = 0;
            if let (Some(start), Some(end)) = (sel_start, sel_end) {
                let sel_center = start.saturating_add(end).saturating_div(2);
                let half = sessions_area.width.saturating_div(2);
                let desired = sel_center.saturating_sub(half);
                let max_scroll = total_left_width.saturating_sub(sessions_area.width);
                scroll_x = desired.min(max_scroll);
            }

            Paragraph::new(vec![sessions_line])
                .scroll((0, scroll_x))
                .render(sessions_area, buf);

            if bar_area.height > 1 {
                let second_line_y = bar_area.y + 1;

                let help_width: u16 = help_line
                    .spans
                    .iter()
                    .map(|s| UnicodeWidthStr::width(s.content.as_ref()) as u16)
                    .sum();

                let help_area = if help_width < bar_area.width {
                    Rect {
                        x: bar_area.x + bar_area.width.saturating_sub(help_width),
                        y: second_line_y,
                        width: help_width,
                        height: 1,
                    }
                } else {
                    Rect {
                        x: bar_area.x,
                        y: second_line_y,
                        width: bar_area.width,
                        height: 1,
                    }
                };
                Paragraph::new(vec![help_line]).render(help_area, buf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn make_session(id: &str, path: PathBuf) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            path,
            cwd: ".".to_string(),
            age: "0s".to_string(),
            mtime: 0,
            message_count: 0,
            last_role: "user".to_string(),
            total_tokens: 0,
            model: "gpt-5.1-codex-max".to_string(),
            tumix: None,
        }
    }

    #[test]
    fn refresh_selection_keeps_nearby_when_deleting_selected_session() -> Result<()> {
        let codex_home = TempDir::new()?;
        let cwd = TempDir::new()?;

        let path = |name: &str| cwd.path().join(format!("{name}.json"));
        let mut bar = SessionBar {
            codex_home: codex_home.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            sessions: vec![
                make_session("A", path("a")),
                make_session("B", path("b")),
                make_session("C", path("c")),
                make_session("D", path("d")),
            ],
            selected_index: 2,
            selected_on_new: false,
            has_focus: true,
            loading: false,
            error: None,
            current_session_id: Some("A".to_string()),
            current_session_status: None,
            label_cache: HashMap::new(),
            alias_manager: SessionAliasManager::load(codex_home.path().to_path_buf()),
        };

        let previous_selected_id = bar.selected_session().map(|s| s.id.clone());
        let previous_selected_index = bar.selected_index;
        let previous_selected_on_new = bar.selected_on_new;

        bar.sessions = vec![
            make_session("A", path("a")),
            make_session("B", path("b")),
            make_session("D", path("d")),
        ];
        bar.reconcile_selection_after_refresh(
            previous_selected_id,
            previous_selected_index,
            previous_selected_on_new,
            true,
        );

        assert_eq!(bar.selected_index, 2);
        assert_eq!(bar.selected_session().map(|s| s.id.as_str()), Some("D"));
        Ok(())
    }

    #[test]
    fn refresh_selection_clamps_when_deleting_last_session() -> Result<()> {
        let codex_home = TempDir::new()?;
        let cwd = TempDir::new()?;

        let path = |name: &str| cwd.path().join(format!("{name}.json"));
        let mut bar = SessionBar {
            codex_home: codex_home.path().to_path_buf(),
            cwd: cwd.path().to_path_buf(),
            sessions: vec![
                make_session("A", path("a")),
                make_session("B", path("b")),
                make_session("C", path("c")),
            ],
            selected_index: 2,
            selected_on_new: false,
            has_focus: true,
            loading: false,
            error: None,
            current_session_id: Some("A".to_string()),
            current_session_status: None,
            label_cache: HashMap::new(),
            alias_manager: SessionAliasManager::load(codex_home.path().to_path_buf()),
        };

        let previous_selected_id = bar.selected_session().map(|s| s.id.clone());
        let previous_selected_index = bar.selected_index;
        let previous_selected_on_new = bar.selected_on_new;

        bar.sessions = vec![make_session("A", path("a")), make_session("B", path("b"))];
        bar.reconcile_selection_after_refresh(
            previous_selected_id,
            previous_selected_index,
            previous_selected_on_new,
            true,
        );

        assert_eq!(bar.selected_index, 1);
        assert_eq!(bar.selected_session().map(|s| s.id.as_str()), Some("B"));
        Ok(())
    }
}
