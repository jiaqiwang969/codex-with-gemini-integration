//! 会话别名输入视图
//!
//! 当创建新会话时，显示一个简单的输入框让用户为会话命名

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
use ratatui::widgets::Widget;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use crate::render::renderable::Renderable;

/// 回调函数类型，当用户提交别名时调用
pub(crate) type AliasSubmitted = Box<dyn Fn(String, String) + Send + Sync>;

/// 会话别名输入视图
pub(crate) struct SessionAliasInput {
    /// 会话ID
    session_id: String,
    /// 用户输入的别名
    input: String,
    /// 提交回调
    on_submit: AliasSubmitted,
    /// 是否完成输入
    complete: bool,
    /// 光标位置
    cursor_position: usize,
    /// 是否是重命名操作（初始时有别名）
    is_rename_mode: bool,
}

impl SessionAliasInput {
    /// 创建新的别名输入视图
    pub(crate) fn new(session_id: String, on_submit: AliasSubmitted) -> Self {
        // 尝试加载现有别名作为默认值
        let alias_manager = crate::session_alias_manager::SessionAliasManager::load();
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

    /// 检查是否是重命名操作（初始时有别名）
    fn is_rename(&self) -> bool {
        self.is_rename_mode
    }

    /// 清理和验证别名
    fn sanitize_alias(alias: &str) -> String {
        let trimmed = alias.trim();

        // 限制长度为 30 个字符
        let truncated: String = if trimmed.chars().count() > 30 {
            trimmed.chars().take(30).collect()
        } else {
            trimmed.to_string()
        };

        // 过滤控制字符，保留常见的字符
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
                // 用户跳过命名
                self.complete = true;
            }
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if self.cursor_position > 0 {
                    // 获取光标前的字符边界
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
                // 限制长度
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
        // 清除背景
        Clear.render(area, buf);

        // 计算内容区域（居中显示）
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

        // 构建显示内容
        let title_text = if self.is_rename() {
            "重命名会话"
        } else {
            "给会话起个名字"
        };
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::from("✨ ").yellow(),
                Span::from(title_text).cyan().bold(),
            ]),
            Line::from(""),
        ];

        // 输入行，包含光标
        let mut input_spans = vec![Span::from("  ")];
        let chars: Vec<char> = self.input.chars().collect();

        // 添加光标前的文本
        if self.cursor_position > 0 {
            let before: String = chars[..self.cursor_position].iter().collect();
            input_spans.push(Span::from(before));
        }

        // 添加光标
        if self.cursor_position < chars.len() {
            // 光标在字符上
            let cursor_char = chars[self.cursor_position].to_string();
            input_spans.push(Span::from(cursor_char).reversed());
        } else {
            // 光标在末尾
            input_spans.push(Span::from("_").reversed());
        }

        // 添加光标后的文本
        if self.cursor_position + 1 < chars.len() {
            let after: String = chars[self.cursor_position + 1..].iter().collect();
            input_spans.push(Span::from(after));
        }

        lines.push(Line::from(input_spans));
        lines.push(Line::from(""));

        // 提示行
        lines.push(Line::from(vec![
            Span::from("  按 ").dim(),
            Span::from("Enter").green().bold(),
            Span::from(" 确认 · ").dim(),
            Span::from("Esc").yellow(),
            Span::from(" 跳过").dim(),
        ]));

        // 渲染
        Paragraph::new(lines).render(content_area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        8 // 弹出窗口高度
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_alias() {
        // 测试正常输入
        assert_eq!(
            SessionAliasInput::sanitize_alias("购物车功能"),
            "购物车功能"
        );

        // 测试去除首尾空格
        assert_eq!(SessionAliasInput::sanitize_alias("  test  "), "test");

        // 测试长度限制
        let long_str = "a".repeat(50);
        let result = SessionAliasInput::sanitize_alias(&long_str);
        assert_eq!(result.chars().count(), 30);

        // 测试空字符串
        assert_eq!(SessionAliasInput::sanitize_alias("   "), "");
    }
}
