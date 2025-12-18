//! Session alias input view.
//!
//! Shown when creating a new session (or renaming an existing session) to let
//! the user provide a friendly alias.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget as _;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use crate::render::renderable::Renderable;
use crate::session_alias_manager::SessionAliasManager;

pub(crate) type AliasSubmitted = Box<dyn Fn(String, String) + Send + Sync>;

pub(crate) struct SessionAliasInput {
    session_id: String,
    input: String,
    on_submit: AliasSubmitted,
    complete: bool,
    cursor_position: usize,
    is_rename_mode: bool,
}

impl SessionAliasInput {
    pub(crate) fn new(codex_home: PathBuf, session_id: String, on_submit: AliasSubmitted) -> Self {
        let alias_manager = SessionAliasManager::load(codex_home);
        let existing_alias = alias_manager.get_alias(&session_id).unwrap_or_default();
        let has_existing = !existing_alias.is_empty();
        let cursor_pos = existing_alias.chars().count();

        Self {
            session_id,
            input: existing_alias,
            on_submit,
            complete: false,
            cursor_position: cursor_pos,
            is_rename_mode: has_existing,
        }
    }

    fn is_rename(&self) -> bool {
        self.is_rename_mode
    }

    fn sanitize_alias(alias: &str) -> String {
        let trimmed = alias.trim();
        let truncated: String = if trimmed.chars().count() > 30 {
            trimmed.chars().take(30).collect()
        } else {
            trimmed.to_string()
        };

        truncated
            .chars()
            .filter(|c| !c.is_control() || *c == ' ')
            .collect()
    }
}

impl BottomPaneView for SessionAliasInput {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                let alias = Self::sanitize_alias(&self.input);
                if !alias.is_empty() {
                    (self.on_submit)(self.session_id.clone(), alias);
                }
                self.complete = true;
            }
            KeyEvent {
                code: KeyCode::Esc, ..
            } => {
                self.complete = true;
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if self.cursor_position > 0 {
                    let mut chars: Vec<char> = self.input.chars().collect();
                    if self.cursor_position <= chars.len() {
                        chars.remove(self.cursor_position - 1);
                        self.input = chars.into_iter().collect();
                        self.cursor_position -= 1;
                    }
                }
            }
            KeyEvent {
                code: KeyCode::Delete,
                ..
            } => {
                let mut chars: Vec<char> = self.input.chars().collect();
                if self.cursor_position < chars.len() {
                    chars.remove(self.cursor_position);
                    self.input = chars.into_iter().collect();
                }
            }
            KeyEvent {
                code: KeyCode::Left,
                ..
            } => {
                if self.cursor_position > 0 {
                    self.cursor_position -= 1;
                }
            }
            KeyEvent {
                code: KeyCode::Right,
                ..
            } => {
                if self.cursor_position < self.input.chars().count() {
                    self.cursor_position += 1;
                }
            }
            KeyEvent {
                code: KeyCode::Home,
                ..
            } => {
                self.cursor_position = 0;
            }
            KeyEvent {
                code: KeyCode::End, ..
            } => {
                self.cursor_position = self.input.chars().count();
            }
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers,
                ..
            } if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT) =>
            {
                if self.input.chars().count() < 30 && !c.is_control() {
                    let mut chars: Vec<char> = self.input.chars().collect();
                    chars.insert(self.cursor_position, c);
                    self.input = chars.into_iter().collect();
                    self.cursor_position += 1;
                }
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.complete = true;
        CancellationEvent::Handled
    }
}

impl Renderable for SessionAliasInput {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let content_height = 6;
        let content_width = 40.min(area.width);
        let start_y = area
            .y
            .saturating_add((area.height.saturating_sub(content_height)) / 2);
        let start_x = area
            .x
            .saturating_add((area.width.saturating_sub(content_width)) / 2);

        let content_area = Rect {
            x: start_x,
            y: start_y,
            width: content_width,
            height: content_height,
        };

        let title_text = if self.is_rename() {
            "Rename session"
        } else {
            "Name this session"
        };
        let mut lines = vec![
            Line::from(""),
            vec![Span::from("✨ ").magenta(), title_text.cyan().bold()].into(),
            Line::from(""),
        ];

        let mut input_spans = vec![Span::from("  ")];
        let chars: Vec<char> = self.input.chars().collect();

        if self.cursor_position > 0 {
            let before: String = chars[..self.cursor_position].iter().collect();
            input_spans.push(before.into());
        }

        if self.cursor_position < chars.len() {
            let cursor_char = chars[self.cursor_position].to_string();
            input_spans.push(cursor_char.reversed());
        } else {
            input_spans.push("_".reversed());
        }

        if self.cursor_position + 1 < chars.len() {
            let after: String = chars[self.cursor_position + 1..].iter().collect();
            input_spans.push(after.into());
        }

        lines.push(input_spans.into());
        lines.push(Line::from(""));

        lines.push(
            vec![
                "  Press ".dim(),
                "Enter".green().bold(),
                " to confirm · ".dim(),
                "Esc".cyan(),
                " to skip".dim(),
            ]
            .into(),
        );

        Paragraph::new(lines).render(content_area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn sanitize_alias_trims_and_limits() {
        assert_eq!(
            SessionAliasInput::sanitize_alias("购物车功能"),
            "购物车功能"
        );
        assert_eq!(SessionAliasInput::sanitize_alias("  test  "), "test");

        let long_str = "a".repeat(50);
        let result = SessionAliasInput::sanitize_alias(&long_str);
        assert_eq!(result.chars().count(), 30);

        assert_eq!(SessionAliasInput::sanitize_alias("   "), "");
    }
}
