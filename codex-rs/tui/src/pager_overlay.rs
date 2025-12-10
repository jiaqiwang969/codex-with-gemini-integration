use std::io::Result;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::cxresume_picker_widget::SessionInfo;
use crate::history_cell::HistoryCell;
use crate::render::line_utils::push_owned_lines;
use crate::tui;
use crate::tui::TuiEvent;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Styled;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;

pub(crate) enum Overlay {
    Transcript(TranscriptOverlay),
    Static(StaticOverlay),
    SessionPicker(Box<SessionPickerOverlay>),
}

/// Session picker overlay integrating PickerState for interactive navigation
pub(crate) struct SessionPickerOverlay {
    picker_state: crate::cxresume_picker_widget::PickerState,
    is_done: bool,
    selected_session_id: Option<String>,
    selected_session: Option<SessionInfo>,
}

impl Overlay {
    pub(crate) fn new_transcript(cells: Vec<Arc<dyn HistoryCell>>) -> Self {
        Self::Transcript(TranscriptOverlay::new(cells))
    }

    pub(crate) fn new_static_with_title(lines: Vec<Line<'static>>, title: String) -> Self {
        Self::Static(StaticOverlay::with_title(lines, title))
    }

    /// Alias for new_static_with_title for compatibility with origin/main
    pub(crate) fn new_static_with_lines(lines: Vec<Line<'static>>, title: String) -> Self {
        Self::new_static_with_title(lines, title)
    }

    #[allow(dead_code)]
    pub(crate) fn new_static_with_title_no_wrap(lines: Vec<Line<'static>>, title: String) -> Self {
        Self::Static(StaticOverlay::with_title_no_wrap(lines, title))
    }

    #[allow(dead_code)]
    pub(crate) fn new_static_with_title_no_wrap_and_path(
        lines: Vec<Line<'static>>,
        title: String,
        repo_path: String,
    ) -> Self {
        Self::Static(StaticOverlay::with_title_no_wrap_and_path(
            lines, title, repo_path,
        ))
    }

    pub(crate) fn new_static_with_title_no_wrap_refresh(
        lines: Vec<Line<'static>>,
        title: String,
        refresh_callback: Box<dyn Fn() -> std::result::Result<Vec<Line<'static>>, String>>,
    ) -> Self {
        Self::Static(StaticOverlay::with_title_no_wrap_refresh(
            lines,
            title,
            refresh_callback,
        ))
    }

    /// Renders renderables to lines and displays them
    /// This is a compatibility shim for origin/main's new API
    pub(crate) fn new_static_with_renderables(
        renderables: Vec<Box<dyn crate::render::renderable::Renderable>>,
        title: String,
    ) -> Self {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        // Render each renderable to a temporary buffer and extract lines
        let mut all_lines: Vec<Line<'static>> = Vec::new();
        let width = 120; // Use a reasonable default width for rendering

        for renderable in renderables {
            let height = renderable.desired_height(width);
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);
            renderable.render(area, &mut buf);

            // Extract lines from buffer
            for y in 0..height {
                let mut line_spans: Vec<ratatui::text::Span<'static>> = Vec::new();
                let mut current_text = String::new();
                let mut current_style = ratatui::style::Style::default();

                for x in 0..width {
                    let cell = &buf[(x, y)];
                    let cell_style = cell.style();

                    if cell_style != current_style && !current_text.is_empty() {
                        line_spans.push(ratatui::text::Span::styled(
                            std::mem::take(&mut current_text),
                            current_style,
                        ));
                        current_style = cell_style;
                    } else if current_style != cell_style {
                        current_style = cell_style;
                    }

                    current_text.push_str(cell.symbol());
                }

                if !current_text.is_empty() {
                    line_spans.push(ratatui::text::Span::styled(current_text, current_style));
                }

                all_lines.push(Line::from(line_spans));
            }
        }

        Self::new_static_with_title(all_lines, title)
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match self {
            Overlay::Transcript(o) => o.handle_event(tui, event),
            Overlay::Static(o) => o.handle_event(tui, event),
            Overlay::SessionPicker(o) => o.handle_event(tui, event),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Overlay::Transcript(o) => o.is_done(),
            Overlay::Static(o) => o.is_done(),
            Overlay::SessionPicker(o) => o.is_done(),
        }
    }

    /// Extract selected session ID if this is a SessionPickerOverlay
    pub(crate) fn get_selected_session_id(&self) -> Option<String> {
        match self {
            Overlay::SessionPicker(o) => o.selected_session_id.clone(),
            _ => None,
        }
    }

    pub(crate) fn get_selected_session(&self) -> Option<SessionInfo> {
        match self {
            Overlay::SessionPicker(o) => o.selected_session_info(),
            _ => None,
        }
    }

    pub(crate) fn session_picker_state(
        &self,
    ) -> Option<crate::cxresume_picker_widget::PickerState> {
        match self {
            Overlay::SessionPicker(o) => Some(o.picker_state.clone()),
            _ => None,
        }
    }
}

// Common pager navigation hints rendered on the first line
const PAGER_KEY_HINTS: &[(&str, &str)] = &[
    ("↑/↓", "scroll"),
    ("PgUp/PgDn", "page"),
    ("Home/End", "jump"),
];

// Render a single line of key hints from (key, description) pairs.
fn render_key_hints(area: Rect, buf: &mut Buffer, pairs: &[(&str, &str)]) {
    let key_hint_style = Style::default().fg(Color::Cyan);
    let mut spans: Vec<Span<'static>> = vec![" ".into()];
    let mut first = true;
    for (key, desc) in pairs {
        if !first {
            spans.push("   ".into());
        }
        spans.push(Span::from(key.to_string()).set_style(key_hint_style));
        spans.push(" ".into());
        spans.push(Span::from(desc.to_string()));
        first = false;
    }
    Paragraph::new(vec![Line::from(spans).dim()]).render_ref(area, buf);
}

/// Generic widget for rendering a pager view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WrapMode {
    WordWrap,
    NoWrap,
}

struct PagerView {
    texts: Vec<Text<'static>>,
    scroll_offset: usize,
    /// Focused wrapped-line index (acts like a cursor).
    cursor_idx: usize,
    title: String,
    wrap_cache: Option<WrapCache>,
    last_content_height: Option<usize>,
    /// If set, on next render ensure this chunk is visible.
    pending_scroll_chunk: Option<usize>,
    // Vim-like navigation/search state
    search_input: Option<String>,
    last_search: Option<String>,
    last_match_idx: Option<usize>,
    g_pending: bool,
    /// Wrapping behavior for rendering: soft wrap or no-wrap.
    wrap_mode: WrapMode,
    /// Horizontal scroll offset (columns) used when `wrap_mode` is NoWrap.
    horiz_offset: usize,
    /// Whether to render a left gutter with a cursor marker on the focused line.
    show_cursor_gutter: bool,
    /// Commit-navigation mode toggle.
    commit_mode: bool,
    /// Current selected commit (wrapped line index, and global column in cells).
    commit_cursor_line: Option<usize>,
    commit_cursor_col: usize,
}

impl PagerView {
    fn new(texts: Vec<Text<'static>>, title: String, scroll_offset: usize) -> Self {
        Self {
            texts,
            scroll_offset,
            cursor_idx: 0,
            title,
            wrap_cache: None,
            last_content_height: None,
            pending_scroll_chunk: None,
            search_input: None,
            last_search: None,
            last_match_idx: None,
            g_pending: false,
            wrap_mode: WrapMode::WordWrap,
            horiz_offset: 0,
            show_cursor_gutter: false,
            commit_mode: false,
            commit_cursor_line: None,
            commit_cursor_col: 0,
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        self.render_header(area, buf);
        let content_area = self.scroll_area(area);
        self.update_last_content_height(content_area.height);
        self.ensure_wrapped(content_area.width);
        // Auto-enter commit mode on first render if requested but not yet positioned
        if self.commit_mode && self.commit_cursor_line.is_none() {
            self.enter_commit_mode(content_area.width, content_area.height);
        }
        // If there is a pending request to scroll a specific chunk into view,
        // satisfy it now that wrapping is up to date for this width.
        if let (Some(idx), Some(cache)) =
            (self.pending_scroll_chunk.take(), self.wrap_cache.as_ref())
            && let Some(range) = cache.chunk_ranges.get(idx).cloned()
        {
            self.ensure_range_visible(range, content_area.height as usize, cache.wrapped.len());
        }
        // Compute page bounds without holding an immutable borrow on cache while mutating self
        let wrapped_len = self
            .wrap_cache
            .as_ref()
            .map(|c| c.wrapped.len())
            .unwrap_or(0);
        self.scroll_offset = self
            .scroll_offset
            .min(wrapped_len.saturating_sub(content_area.height as usize));
        // Clamp cursor to valid range and ensure it's visible by adjusting scroll if needed.
        if wrapped_len == 0 {
            self.cursor_idx = 0;
        } else if self.commit_mode || self.show_cursor_gutter {
            if self.cursor_idx >= wrapped_len {
                self.cursor_idx = wrapped_len - 1;
            }
            self.ensure_cursor_visible(content_area.height as usize);
        }
        let start = self.scroll_offset;
        let end = (start + content_area.height as usize).min(wrapped_len);

        let wrapped = self.cached();
        let page = &wrapped[start..end];
        self.render_content_page_prepared(content_area, buf, start, page);
        self.render_bottom_bar(area, content_area, buf, wrapped);
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        Span::from("/ ".repeat(area.width as usize / 2))
            .dim()
            .render_ref(area, buf);
        let header = format!("/ {}", self.title);
        header.dim().render_ref(area, buf);
    }

    // Removed unused render_content_page (replaced by render_content_page_prepared)

    fn render_content_page_prepared(
        &self,
        area: Rect,
        buf: &mut Buffer,
        page_start: usize,
        page: &[Line<'static>],
    ) {
        Clear.render(area, buf);
        // Horizontal clipping when in NoWrap mode, accounting for an optional left gutter.
        let gutter_cols: u16 = if self.show_cursor_gutter { 2 } else { 0 };
        let mut clipped: Vec<Line<'static>> = if self.wrap_mode == WrapMode::NoWrap {
            let content_width = area.width.saturating_sub(gutter_cols).max(1) as usize;
            page.iter()
                .map(|l| self.clip_line(l, self.horiz_offset, content_width))
                .collect()
        } else {
            page.to_vec()
        };

        // In commit mode, decorate the selected commit by replacing the dot with '◉'.
        if self.commit_mode
            && let Some(cl) = self.commit_cursor_line
            && cl >= page_start
            && cl < page_start + clipped.len()
        {
            let vis_idx = cl - page_start;
            let content_width = area.width.saturating_sub(gutter_cols).max(1) as usize;
            let rel_col = self
                .commit_cursor_col
                .saturating_sub(self.horiz_offset)
                .min(content_width.saturating_sub(1));
            clipped[vis_idx] = Self::decorate_commit_in_line(&clipped[vis_idx], rel_col);
        }

        // Optionally prefix a gutter marker ("▸ ") on the focused line; otherwise two spaces.
        let lines: Vec<Line<'static>> = if self.show_cursor_gutter {
            clipped
                .into_iter()
                .enumerate()
                .map(|(i, mut l)| {
                    let is_cursor = self.cursor_idx == page_start + i;
                    let mut spans = Vec::with_capacity(l.spans.len() + 1);
                    let pref = if is_cursor { "▸ " } else { "  " };
                    spans.push(pref.into());
                    spans.append(&mut l.spans);
                    Line::from(spans).style(l.style)
                })
                .collect()
        } else {
            clipped
        };
        Paragraph::new(lines).render_ref(area, buf);

        let visible = page.len();
        if visible < area.height as usize {
            for i in 0..(area.height as usize - visible) {
                let add = ((visible + i).min(u16::MAX as usize)) as u16;
                let y = area.y.saturating_add(add);
                Span::from("~")
                    .dim()
                    .render_ref(Rect::new(area.x, y, 1, 1), buf);
            }
        }
    }

    fn render_bottom_bar(
        &self,
        full_area: Rect,
        content_area: Rect,
        buf: &mut Buffer,
        wrapped: &[Line<'static>],
    ) {
        let sep_y = content_area.bottom();
        let sep_rect = Rect::new(full_area.x, sep_y, full_area.width, 1);

        Span::from("─".repeat(sep_rect.width as usize))
            .dim()
            .render_ref(sep_rect, buf);
        let percent = if wrapped.is_empty() {
            100
        } else {
            let max_scroll = wrapped.len().saturating_sub(content_area.height as usize);
            if max_scroll == 0 {
                100
            } else {
                (((self.scroll_offset.min(max_scroll)) as f32 / max_scroll as f32) * 100.0).round()
                    as u8
            }
        };
        let pct_text = format!(" {percent}% ");
        let pct_w = pct_text.chars().count() as u16;
        let pct_x = sep_rect.x + sep_rect.width - pct_w - 1;
        Span::from(pct_text)
            .dim()
            .render_ref(Rect::new(pct_x, sep_rect.y, pct_w, 1), buf);

        // If in "/"-search entry mode, show the prompt on the left side.
        if let Some(q) = &self.search_input {
            let prompt = format!("/{q}");
            let max_w = sep_rect.width.saturating_sub(pct_w).saturating_sub(2);
            let w = (prompt.chars().count() as u16).min(max_w);
            if w > 0 {
                Span::from(prompt)
                    .cyan()
                    .render_ref(Rect::new(sep_rect.x + 1, sep_rect.y, w, 1), buf);
            }
        }
    }

    fn handle_key_event(&mut self, tui: &mut tui::Tui, key_event: KeyEvent) -> Result<()> {
        // Ensure wrapping exists for current viewport; required for search/jumps
        let area = self.scroll_area(tui.terminal.viewport_area);
        self.ensure_wrapped(area.width);

        // If in search entry, handle input first
        if let Some(buf) = &mut self.search_input {
            match key_event {
                KeyEvent {
                    code: KeyCode::Esc,
                    kind: KeyEventKind::Press | KeyEventKind::Repeat,
                    ..
                } => {
                    self.search_input = None;
                    tui.frame_requester()
                        .schedule_frame_in(Duration::from_millis(16));
                    return Ok(());
                }
                KeyEvent {
                    code: KeyCode::Enter,
                    kind: KeyEventKind::Press,
                    ..
                } => {
                    let q = buf.trim().to_string();
                    self.search_input = None;
                    if !q.is_empty() {
                        self.last_search = Some(q.clone());
                        let start = self.scroll_offset.min(self.cached().len());
                        if let Some(idx) = self.find_next_match(&q, start) {
                            self.cursor_idx = idx;
                            self.center_on(idx, area.height as usize);
                            self.last_match_idx = Some(idx);
                        }
                    }
                    tui.frame_requester()
                        .schedule_frame_in(Duration::from_millis(16));
                    return Ok(());
                }
                KeyEvent {
                    code: KeyCode::Backspace,
                    kind: KeyEventKind::Press | KeyEventKind::Repeat,
                    ..
                } => {
                    buf.pop();
                    tui.frame_requester()
                        .schedule_frame_in(Duration::from_millis(16));
                    return Ok(());
                }
                KeyEvent {
                    code: KeyCode::Char(c),
                    kind: KeyEventKind::Press | KeyEventKind::Repeat,
                    ..
                } => {
                    if !c.is_control() {
                        buf.push(c);
                        tui.frame_requester()
                            .schedule_frame_in(Duration::from_millis(16));
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        match key_event {
            // Ignore Enter/Esc for mode toggling; commit mode is active by default in Git Graph overlay
            KeyEvent {
                code: KeyCode::Enter | KeyCode::Esc,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                return Ok(());
            }
            KeyEvent {
                code: KeyCode::Up,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('k'),
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if self.commit_mode {
                    self.move_commit_vertical(-1, area.width, area.height);
                } else {
                    if self.cursor_idx > 0 {
                        self.cursor_idx -= 1;
                    }
                    self.ensure_cursor_visible(area.height as usize);
                }
                self.g_pending = false;
            }
            KeyEvent {
                code: KeyCode::Down,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            }
            | KeyEvent {
                code: KeyCode::Char('j'),
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if self.commit_mode {
                    self.move_commit_vertical(1, area.width, area.height);
                } else {
                    if let Some(cache) = self.wrap_cache.as_ref()
                        && self.cursor_idx + 1 < cache.wrapped.len()
                    {
                        self.cursor_idx += 1;
                    }
                    self.ensure_cursor_visible(area.height as usize);
                }
                self.g_pending = false;
            }
            // Horizontal scroll or commit-branch navigation
            KeyEvent {
                code: KeyCode::Char('h'),
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if self.commit_mode {
                    self.move_commit_horizontal(-1, area.width, area.height);
                } else {
                    self.horiz_offset = self.horiz_offset.saturating_sub(1);
                }
            }
            KeyEvent {
                code: KeyCode::Char('l'),
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if self.commit_mode {
                    self.move_commit_horizontal(1, area.width, area.height);
                } else {
                    self.horiz_offset = self.horiz_offset.saturating_add(1);
                }
            }
            KeyEvent {
                code: KeyCode::Char('0'),
                kind: KeyEventKind::Press,
                ..
            } => {
                self.horiz_offset = 0;
            }
            // Vim-like jumps: gg to top, G to bottom
            KeyEvent {
                code: KeyCode::Char('g'),
                kind: KeyEventKind::Press,
                ..
            } => {
                if self.g_pending {
                    self.g_pending = false;
                    // Auto-activate commit mode and jump to first commit
                    if let Some(cache) = self.wrap_cache.as_ref() {
                        // Find first line with commits
                        if let Some((first_line, cols)) = cache
                            .commit_cols
                            .iter()
                            .enumerate()
                            .find(|(_, cols)| !cols.is_empty())
                        {
                            self.commit_mode = true; // Activate commit mode
                            self.commit_cursor_line = Some(first_line);
                            self.commit_cursor_col = cols[0];
                            self.ensure_commit_visible(area.width, area.height);
                        } else {
                            // No commits found, fallback to normal mode
                            self.cursor_idx = 0;
                            self.ensure_cursor_visible(area.height as usize);
                            self.last_match_idx = Some(0);
                        }
                    }
                } else {
                    self.g_pending = true;
                }
            }
            KeyEvent {
                code: KeyCode::Char('G'),
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                self.g_pending = false;
                if let Some(cache) = self.wrap_cache.as_ref()
                    && !cache.wrapped.is_empty()
                {
                    // Auto-activate commit mode and jump to last commit
                    // Find last line with commits
                    if let Some((last_line, cols)) = cache
                        .commit_cols
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, cols)| !cols.is_empty())
                    {
                        self.commit_mode = true; // Activate commit mode
                        self.commit_cursor_line = Some(last_line);
                        self.commit_cursor_col = cols[0];
                        self.ensure_commit_visible(area.width, area.height);
                    } else {
                        // No commits found, fallback to normal mode
                        self.cursor_idx = cache.wrapped.len() - 1;
                        self.ensure_cursor_visible(area.height as usize);
                        self.last_match_idx = Some(self.cursor_idx);
                    }
                }
            }
            KeyEvent {
                code: KeyCode::PageUp,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                let page = area.height as usize;
                if self.commit_mode {
                    self.move_to_nearby_commit(-(page as isize) as i32, 0, area.width, area.height);
                } else {
                    self.cursor_idx = self.cursor_idx.saturating_sub(page);
                    self.ensure_cursor_visible(page);
                }
                self.g_pending = false;
            }
            KeyEvent {
                code: KeyCode::PageDown | KeyCode::Char(' '),
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if let Some(cache) = self.wrap_cache.as_ref() {
                    let page = area.height as usize;
                    let last = cache.wrapped.len().saturating_sub(1);
                    if self.commit_mode {
                        self.move_to_nearby_commit(page as i32, 0, area.width, area.height);
                    } else {
                        self.cursor_idx = (self.cursor_idx + page).min(last);
                        self.ensure_cursor_visible(page);
                    }
                }
                self.g_pending = false;
            }
            KeyEvent {
                code: KeyCode::Home,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                self.cursor_idx = 0;
                self.commit_cursor_line = None;
                self.commit_mode = false;
                self.ensure_cursor_visible(area.height as usize);
                self.g_pending = false;
            }
            KeyEvent {
                code: KeyCode::End,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if let Some(cache) = self.wrap_cache.as_ref()
                    && !cache.wrapped.is_empty()
                {
                    self.cursor_idx = cache.wrapped.len() - 1;
                    self.commit_cursor_line = None;
                    self.commit_mode = false;
                    self.ensure_cursor_visible(area.height as usize);
                }
                self.g_pending = false;
            }
            // Enter search mode with '/'; then 'Enter' to confirm; 'n'/'N' to navigate.
            KeyEvent {
                code: KeyCode::Char('/'),
                kind: KeyEventKind::Press,
                ..
            } => {
                self.g_pending = false;
                self.search_input = Some(String::new());
            }
            KeyEvent {
                code: KeyCode::Char('n'),
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if let Some(q) = self.last_search.clone() {
                    let start = self
                        .last_match_idx
                        .map(|i| i.saturating_add(1))
                        .unwrap_or(self.scroll_offset);
                    if let Some(idx) = self.find_next_match(&q, start) {
                        self.cursor_idx = idx;
                        self.center_on(idx, area.height as usize);
                        self.last_match_idx = Some(idx);
                    }
                }
                self.g_pending = false;
            }
            KeyEvent {
                code: KeyCode::Char('N'),
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                if let Some(q) = self.last_search.clone() {
                    let start = self.last_match_idx.unwrap_or(self.scroll_offset);
                    if let Some(idx) = self.find_prev_match(&q, start) {
                        self.cursor_idx = idx;
                        self.center_on(idx, area.height as usize);
                        self.last_match_idx = Some(idx);
                    }
                }
                self.g_pending = false;
            }
            _ => {
                self.g_pending = false;
                return Ok(());
            }
        }
        tui.frame_requester()
            .schedule_frame_in(Duration::from_millis(16));
        Ok(())
    }

    /// Returns the height of one page in content rows.
    ///
    /// Prefers the last rendered content height (excluding header/footer chrome);
    /// if no render has occurred yet, falls back to the content area height
    /// computed from the given viewport.
    fn page_height(&self, viewport_area: Rect) -> usize {
        self.last_content_height
            .unwrap_or_else(|| self.content_area(viewport_area).height as usize)
    }

    fn update_last_content_height(&mut self, height: u16) {
        self.last_content_height = Some(height as usize);
    }

    fn scroll_area(&self, area: Rect) -> Rect {
        let mut area = area;
        area.y = area.y.saturating_add(1);
        area.height = area.height.saturating_sub(2);
        area
    }

    fn content_area(&self, area: Rect) -> Rect {
        self.scroll_area(area)
    }
}

#[derive(Debug, Clone)]
struct WrapCache {
    width: u16,
    wrapped: Vec<Line<'static>>,
    /// For each input Text chunk, the inclusive-excluded range of wrapped lines produced.
    chunk_ranges: Vec<std::ops::Range<usize>>,
    base_len: usize,
    /// Plain text for wrapped lines, used for searches.
    wrapped_plain: Vec<String>,
    /// Column positions of commit nodes per wrapped line (in cells, pre-clip).
    commit_cols: Vec<Vec<usize>>,
}

impl PagerView {
    fn ensure_wrapped(&mut self, width: u16) {
        let width = width.max(1);
        let needs = match self.wrap_cache {
            Some(ref c) => c.width != width || c.base_len != self.texts.len(),
            None => true,
        };
        if !needs {
            return;
        }
        let mut wrapped: Vec<Line<'static>> = Vec::new();
        let mut wrapped_plain: Vec<String> = Vec::new();
        let mut commit_cols: Vec<Vec<usize>> = Vec::new();
        let mut chunk_ranges: Vec<std::ops::Range<usize>> = Vec::with_capacity(self.texts.len());
        for text in &self.texts {
            let start = wrapped.len();
            for line in &text.lines {
                match self.wrap_mode {
                    WrapMode::WordWrap => {
                        let ws = crate::wrapping::word_wrap_line(line, width as usize);
                        push_owned_lines(&ws, &mut wrapped);
                        for l in &ws {
                            let p = Self::plain_text(l);
                            wrapped_plain.push(p.clone());
                            commit_cols.push(Self::scan_commit_cols(&p));
                        }
                    }
                    WrapMode::NoWrap => {
                        // Do not wrap; use the line as-is (owned). Horizontal clipping is applied at render time.
                        push_owned_lines(std::slice::from_ref(line), &mut wrapped);
                        let p = Self::plain_text(line);
                        wrapped_plain.push(p.clone());
                        commit_cols.push(Self::scan_commit_cols(&p));
                    }
                }
            }
            let end = wrapped.len();
            chunk_ranges.push(start..end);
        }
        self.wrap_cache = Some(WrapCache {
            width,
            wrapped,
            chunk_ranges,
            base_len: self.texts.len(),
            wrapped_plain,
            commit_cols,
        });
    }

    fn cached(&self) -> &[Line<'static>] {
        if let Some(cache) = self.wrap_cache.as_ref() {
            &cache.wrapped
        } else {
            &[]
        }
    }

    fn is_scrolled_to_bottom(&self) -> bool {
        if self.scroll_offset == usize::MAX {
            return true;
        }
        let Some(cache) = &self.wrap_cache else {
            return false;
        };
        let Some(height) = self.last_content_height else {
            return false;
        };
        if cache.wrapped.is_empty() {
            return true;
        }
        let visible = height.min(cache.wrapped.len());
        let max_scroll = cache.wrapped.len().saturating_sub(visible);
        self.scroll_offset >= max_scroll
    }

    /// Request that the given text chunk index be scrolled into view on next render.
    fn scroll_chunk_into_view(&mut self, chunk_index: usize) {
        self.pending_scroll_chunk = Some(chunk_index);
    }

    fn ensure_range_visible(
        &mut self,
        range: std::ops::Range<usize>,
        viewport_height: usize,
        total_wrapped: usize,
    ) {
        if viewport_height == 0 || total_wrapped == 0 {
            return;
        }
        let first = range.start.min(total_wrapped.saturating_sub(1));
        let last = range
            .end
            .saturating_sub(1)
            .min(total_wrapped.saturating_sub(1));
        let current_top = self.scroll_offset.min(total_wrapped.saturating_sub(1));
        let current_bottom = current_top.saturating_add(viewport_height.saturating_sub(1));

        if first < current_top {
            self.scroll_offset = first;
        } else if last > current_bottom {
            // Scroll just enough so that 'last' is visible at the bottom
            self.scroll_offset = last.saturating_sub(viewport_height.saturating_sub(1));
        }
    }

    /// Convert a styled Line into a plain string for searching.
    fn plain_text(line: &Line<'_>) -> String {
        let mut s = String::new();
        for sp in &line.spans {
            s.push_str(sp.content.as_ref());
        }
        s
    }

    /// Return column positions (cells) of commit dots in a plain string.
    fn scan_commit_cols(s: &str) -> Vec<usize> {
        use unicode_width::UnicodeWidthChar;
        let mut cols = Vec::new();
        let mut col = 0usize;
        for ch in s.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
            if ch == '●' || ch == '○' {
                cols.push(col);
            }
            col += w;
        }
        cols
    }

    /// Center the viewport on a given wrapped line index.
    fn center_on(&mut self, idx: usize, viewport_height: usize) {
        if let Some(cache) = &self.wrap_cache {
            let total = cache.wrapped.len();
            if total == 0 {
                self.scroll_offset = 0;
                return;
            }
            let vis = viewport_height.min(total);
            let half = vis / 3; // bias to upper third for context
            let base = idx.saturating_sub(half);
            let max_scroll = total.saturating_sub(vis);
            self.scroll_offset = base.min(max_scroll);
        }
    }

    fn find_next_match(&self, q: &str, start: usize) -> Option<usize> {
        let cache = self.wrap_cache.as_ref()?;
        if q.is_empty() {
            return None;
        }
        let smart_case = q.chars().any(char::is_uppercase);
        if smart_case {
            for (i, line) in cache.wrapped_plain.iter().enumerate().skip(start) {
                if line.contains(q) {
                    return Some(i);
                }
            }
        } else {
            let ql = q.to_lowercase();
            for (i, line) in cache.wrapped_plain.iter().enumerate().skip(start) {
                if line.to_lowercase().contains(&ql) {
                    return Some(i);
                }
            }
        }
        None
    }

    fn find_prev_match(&self, q: &str, start: usize) -> Option<usize> {
        let cache = self.wrap_cache.as_ref()?;
        if q.is_empty() {
            return None;
        }
        let end = start.min(cache.wrapped_plain.len());
        let smart_case = q.chars().any(char::is_uppercase);
        if smart_case {
            for i in (0..end).rev() {
                if cache.wrapped_plain[i].contains(q) {
                    return Some(i);
                }
            }
        } else {
            let ql = q.to_lowercase();
            for i in (0..end).rev() {
                if cache.wrapped_plain[i].to_lowercase().contains(&ql) {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Ensure the cursor is visible within the current viewport height; adjust scroll if needed.
    fn ensure_cursor_visible(&mut self, viewport_height: usize) {
        if let Some(cache) = &self.wrap_cache {
            if cache.wrapped.is_empty() || viewport_height == 0 {
                return;
            }
            let total = cache.wrapped.len();
            let top = self.scroll_offset.min(total.saturating_sub(1));
            let bottom = top.saturating_add(viewport_height.saturating_sub(1));
            if self.cursor_idx < top {
                self.scroll_offset = self.cursor_idx;
            } else if self.cursor_idx > bottom {
                self.scroll_offset = self
                    .cursor_idx
                    .saturating_sub(viewport_height.saturating_sub(1));
            }
        }
    }

    /// Toggle into commit mode will call this to select nearest commit; already implemented above.
    /// Move vertically along the branch route by following connecting glyphs.
    fn move_commit_vertical(&mut self, dir: i32, viewport_width: u16, viewport_height: u16) {
        if !self.commit_mode {
            return;
        }
        if self.commit_cursor_line.is_none() {
            self.enter_commit_mode(viewport_width, viewport_height);
        }
        let Some(mut line) = self.commit_cursor_line else {
            return;
        };
        let mut col = self.commit_cursor_col;
        let step: i32 = if dir >= 0 { 1 } else { -1 };
        // perform scanning in a limited-scope borrow to avoid conflicts with later &mut self calls
        let (mut i, limit_low, limit_high) = {
            let cache = match &self.wrap_cache {
                Some(c) => c,
                None => return,
            };
            (
                line as i64 + step as i64,
                0i64,
                cache.wrapped_plain.len() as i64 - 1,
            )
        };
        let mut advanced = false;
        while i >= limit_low && i <= limit_high {
            let li = i as usize;
            // Adjust column based on connector glyphs on this line, with small lookaround + 1–2 line lookahead
            let (cand_m1, cand_0, cand_p1) = {
                let cache = match &self.wrap_cache {
                    Some(c) => c,
                    None => return,
                };
                (
                    Self::char_at_cell(&cache.wrapped_plain[li], col.saturating_sub(1)),
                    Self::char_at_cell(&cache.wrapped_plain[li], col),
                    Self::char_at_cell(&cache.wrapped_plain[li], col.saturating_add(1)),
                )
            };
            let mut dcol: i32 = 0;
            let pref = |c: Option<char>| c.unwrap_or(' ');
            let c0 = pref(cand_0);
            let c_l = pref(cand_m1);
            let c_r = pref(cand_p1);
            // Primary rules by glyph; otherwise evaluate candidates by lookahead scoring
            let mut ambiguous = false;
            match c0 {
                '│' | '╭' | '╮' | '╰' | '╯' => dcol = 0,
                '╱' => dcol = if dir > 0 { 1 } else { -1 },
                '╲' => dcol = if dir > 0 { -1 } else { 1 },
                '┼' | '─' | ' ' => ambiguous = true,
                _ => ambiguous = true,
            }
            if ambiguous {
                let best = self.choose_dcol_with_lookahead(li, col, dir, 2 /*lines*/);
                dcol = best;
                if dcol == 0 {
                    // still ambiguous: use adjacent hints
                    if dir > 0 {
                        if c_r == '╱' {
                            dcol = 1;
                        } else if c_l == '╲' {
                            dcol = -1;
                        }
                    } else if c_l == '╱' {
                        dcol = -1;
                    } else if c_r == '╲' {
                        dcol = 1;
                    }
                }
            }
            if dcol < 0 {
                col = col.saturating_sub(1);
            } else if dcol > 0 {
                col = col.saturating_add(1);
            }
            // If this line has a commit at current col, stop
            let has_commit_here = {
                let cache = match &self.wrap_cache {
                    Some(c) => c,
                    None => return,
                };
                cache
                    .commit_cols
                    .get(li)
                    .map(|cols| cols.contains(&col))
                    .unwrap_or(false)
            };
            if has_commit_here {
                line = li;
                advanced = true;
                break;
            }
            i += step as i64;
        }
        if advanced {
            self.commit_cursor_line = Some(line);
            self.commit_cursor_col = col;
            self.ensure_commit_visible(viewport_width, viewport_height);
        } else {
            // fallback to nearest logic if not found
            self.move_to_nearby_commit(
                if dir > 0 { 1 } else { -1 },
                0,
                viewport_width,
                viewport_height,
            );
        }
    }

    /// Choose initial horizontal delta (-1,0,1) by simulating up to `lookahead` lines and
    /// selecting the path that reaches a commit in fewer steps; on ties, prefer straight (0), then smaller lateral.
    fn choose_dcol_with_lookahead(
        &self,
        line_idx: usize,
        col: usize,
        dir: i32,
        lookahead: usize,
    ) -> i32 {
        let mut best_dcol = 0;
        let mut best_steps = usize::MAX;
        let mut best_lateral = usize::MAX;
        for &dcol in &[-1, 0, 1] {
            let (steps, lateral) = self.simulate_to_commit(line_idx, col, dir, dcol, lookahead);
            if steps < best_steps
                || (steps == best_steps
                    && (lateral < best_lateral || (lateral == best_lateral && dcol == 0)))
            {
                best_steps = steps;
                best_lateral = lateral;
                best_dcol = dcol;
            }
        }
        best_dcol
    }

    /// Simulate advancing from (line_idx, col) with an initial dcol, for up to `lookahead` lines.
    /// Return (steps_to_commit_or_max, total_lateral_displacement).
    fn simulate_to_commit(
        &self,
        line_idx: usize,
        col: usize,
        dir: i32,
        init_dcol: i32,
        lookahead: usize,
    ) -> (usize, usize) {
        let mut col_cur = if init_dcol < 0 {
            col.saturating_sub(1)
        } else if init_dcol > 0 {
            col.saturating_add(1)
        } else {
            col
        };
        let mut lateral = col_cur.abs_diff(col);
        for step in 1..=lookahead {
            let li = if dir > 0 {
                line_idx + step
            } else if line_idx >= step {
                line_idx - step
            } else {
                return (usize::MAX, lateral);
            };
            let cache = match &self.wrap_cache {
                Some(c) => c,
                None => return (usize::MAX, lateral),
            };
            if li >= cache.wrapped_plain.len() {
                return (usize::MAX, lateral);
            }
            // Adjust column based on glyph at this line
            if let Some(ch) = Self::char_at_cell(&cache.wrapped_plain[li], col_cur) {
                match ch {
                    '╱' => {
                        col_cur = if dir > 0 {
                            col_cur.saturating_add(1)
                        } else {
                            col_cur.saturating_sub(1)
                        };
                    }
                    '╲' => {
                        col_cur = if dir > 0 {
                            col_cur.saturating_sub(1)
                        } else {
                            col_cur.saturating_add(1)
                        };
                    }
                    _ => {}
                }
                lateral = lateral.max(col_cur.abs_diff(col));
            }
            if cache
                .commit_cols
                .get(li)
                .map(|cols| cols.contains(&col_cur))
                .unwrap_or(false)
            {
                return (step, lateral);
            }
        }
        (usize::MAX, lateral)
    }

    /// Find char at visual column index in the given plain string.
    fn char_at_cell(s: &str, target: usize) -> Option<char> {
        use unicode_width::UnicodeWidthChar;
        let mut col = 0usize;
        for ch in s.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
            if col == target {
                return Some(ch);
            }
            if target < col + w {
                return Some(ch);
            }
            col += w;
        }
        None
    }

    fn ensure_commit_visible(&mut self, viewport_width: u16, viewport_height: u16) {
        if !self.commit_mode {
            return;
        }
        let Some(line) = self.commit_cursor_line else {
            return;
        };
        self.cursor_idx = line;
        self.ensure_cursor_visible(viewport_height as usize);
        // Adjust horizontal offset so that the commit column is visible.
        let content_width =
            viewport_width.saturating_sub(if self.show_cursor_gutter { 2 } else { 0 });
        if content_width == 0 {
            return;
        }
        let right_edge = self.horiz_offset + content_width as usize - 1;
        if self.commit_cursor_col < self.horiz_offset {
            self.horiz_offset = self.commit_cursor_col;
        } else if self.commit_cursor_col > right_edge {
            self.horiz_offset = self
                .commit_cursor_col
                .saturating_sub(content_width as usize - 1);
        }
    }

    fn enter_commit_mode(&mut self, viewport_width: u16, viewport_height: u16) {
        self.commit_mode = true;
        // Pick nearest commit in the current viewport; fallback to anywhere.
        let Some(cache) = &self.wrap_cache else {
            return;
        };
        let top = self.scroll_offset;
        let bottom = top
            .saturating_add(viewport_height as usize)
            .saturating_sub(1);
        let mut best: Option<(usize, usize)> = None; // (line, distance)
        for i in top..=bottom.min(cache.commit_cols.len().saturating_sub(1)) {
            if !cache
                .commit_cols
                .get(i)
                .map(|v| !v.is_empty())
                .unwrap_or(false)
            {
                continue;
            }
            let dist = self.cursor_idx.abs_diff(i);
            if best.map(|(_, d)| dist < d).unwrap_or(true) {
                best = Some((i, dist));
            }
        }
        if best.is_none() {
            for i in 0..cache.commit_cols.len() {
                if cache
                    .commit_cols
                    .get(i)
                    .map(|v| !v.is_empty())
                    .unwrap_or(false)
                {
                    best = Some((i, self.cursor_idx.abs_diff(i)));
                    break;
                }
            }
        }
        if let Some((line, _)) = best {
            self.commit_cursor_line = Some(line);
            // Choose a column: nearest to previous commit col if any; otherwise first
            let cols = cache.commit_cols.get(line).cloned().unwrap_or_default();
            if cols.is_empty() {
                self.commit_cursor_col = 0;
            } else if self.commit_cursor_col == 0 {
                self.commit_cursor_col = cols[0];
            } else {
                // nearest to previous
                let mut bestc = cols[0];
                let mut bestd = self.commit_cursor_col.abs_diff(bestc);
                for c in cols.into_iter().skip(1) {
                    let d = self.commit_cursor_col.abs_diff(c);
                    if d < bestd {
                        bestd = d;
                        bestc = c;
                    }
                }
                self.commit_cursor_col = bestc;
            }
            self.ensure_commit_visible(viewport_width, viewport_height);
        } else {
            // No commits; leave mode
            self.commit_mode = false;
            self.commit_cursor_line = None;
        }
    }

    /// Move commit selection by delta lines/columns; adjust visibility accordingly.
    fn move_to_nearby_commit(
        &mut self,
        delta_lines: i32,
        delta_cols: i32,
        viewport_width: u16,
        viewport_height: u16,
    ) {
        let Some(cache) = &self.wrap_cache else {
            return;
        };
        if !self.commit_mode {
            return;
        }
        // Initialize from cursor if unset
        let mut line = self.commit_cursor_line.unwrap_or(self.cursor_idx);
        let mut col = self.commit_cursor_col;

        if delta_lines != 0 {
            // Move to next/prev line that has commits; choose nearest column.
            let mut i = line as i64 + delta_lines as i64;
            while i >= 0 && (i as usize) < cache.commit_cols.len() {
                let li = i as usize;
                if let Some(cols) = cache.commit_cols.get(li)
                    && !cols.is_empty()
                {
                    // pick nearest column
                    let mut bestc = cols[0];
                    let mut bestd = col.abs_diff(bestc);
                    for &c in cols.iter().skip(1) {
                        let d = col.abs_diff(c);
                        if d < bestd {
                            bestd = d;
                            bestc = c;
                        }
                    }
                    line = li;
                    col = bestc;
                    break;
                }
                i += delta_lines as i64;
            }
        }

        if delta_cols != 0 {
            // Move left/right among commits on the same line.
            if let Some(cols) = cache.commit_cols.get(line)
                && !cols.is_empty()
            {
                // find current position index
                let mut idx = 0usize;
                for (k, &c) in cols.iter().enumerate() {
                    if c >= col {
                        idx = k;
                        break;
                    }
                    idx = k;
                }
                if delta_cols < 0 {
                    idx = idx.saturating_sub(1);
                } else if delta_cols > 0 && idx + 1 < cols.len() {
                    idx += 1;
                }
                col = cols[idx];
            }
        }

        self.commit_cursor_line = Some(line);
        self.commit_cursor_col = col;
        self.ensure_commit_visible(viewport_width, viewport_height);
    }

    /// Move to nearest commit on an adjacent branch to the left (dir < 0) or right (dir > 0).
    /// Preference: minimize horizontal distance first, then vertical distance from the current commit.
    fn move_commit_horizontal(&mut self, dir: i32, viewport_width: u16, viewport_height: u16) {
        if !self.commit_mode {
            return;
        }
        if self.commit_cursor_line.is_none() {
            self.enter_commit_mode(viewport_width, viewport_height);
        }
        let Some(line0) = self.commit_cursor_line else {
            return;
        };
        let col0 = self.commit_cursor_col;
        let mut best_line: Option<usize> = None;
        let mut best_col: usize = 0;
        let mut best_dx: usize = usize::MAX;
        let mut best_dy: usize = usize::MAX;
        if let Some(cache) = &self.wrap_cache {
            for (li, cols) in cache.commit_cols.iter().enumerate() {
                for &c in cols {
                    if (dir < 0 && c < col0) || (dir > 0 && c > col0) {
                        let dx = c.abs_diff(col0);
                        if dx == 0 {
                            continue;
                        }
                        let dy = li.abs_diff(line0);
                        if dx < best_dx || (dx == best_dx && dy < best_dy) {
                            best_dx = dx;
                            best_dy = dy;
                            best_line = Some(li);
                            best_col = c;
                        }
                    }
                }
            }
        }
        if let Some(li) = best_line {
            self.commit_cursor_line = Some(li);
            self.commit_cursor_col = best_col;
            self.ensure_commit_visible(viewport_width, viewport_height);
        }
    }

    /// Clip a styled line horizontally given a starting column and width; preserves styles.
    fn clip_line(&self, line: &Line<'_>, start_col: usize, width: usize) -> Line<'static> {
        use ratatui::text::Span;
        use unicode_width::UnicodeWidthChar;

        if width == 0 {
            return Line::default();
        }
        let mut out_spans: Vec<Span<'static>> = Vec::new();
        let mut col = 0usize;
        let mut taken = 0usize;
        let mut started = false;
        for s in &line.spans {
            let text = s.content.as_ref();
            if text.is_empty() {
                continue;
            }
            let mut buf = String::new();
            for ch in text.chars() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
                if !started {
                    if col + w > start_col {
                        started = true;
                    } else {
                        col += w;
                        continue;
                    }
                }
                if taken + w > width {
                    break;
                }
                buf.push(ch);
                taken += w;
                col += w;
            }
            if !buf.is_empty() {
                out_spans.push(Span::from(buf).set_style(s.style));
            }
            if taken >= width {
                break;
            }
        }
        Line::from(out_spans).style(line.style)
    }

    /// Replace a commit dot at the given visible column with '◉' (white), preserving surrounding styles.
    fn decorate_commit_in_line(line: &Line<'_>, vis_col: usize) -> Line<'static> {
        use ratatui::text::Span;
        use unicode_width::UnicodeWidthChar;
        let mut out_spans: Vec<Span<'static>> = Vec::new();
        let mut col = 0usize;
        for s in &line.spans {
            let text = s.content.as_ref();
            if text.is_empty() {
                continue;
            }
            let mut buf = String::new();
            for ch in text.chars() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
                if col == vis_col && (ch == '●' || ch == '○') {
                    // Flush existing buffer
                    if !buf.is_empty() {
                        out_spans.push(Span::from(buf.clone()).set_style(s.style));
                        buf.clear();
                    }
                    out_spans.push(Span::from("◉").bold());
                } else {
                    buf.push(ch);
                }
                col += w;
            }
            if !buf.is_empty() {
                out_spans.push(Span::from(buf).set_style(s.style));
            }
        }
        Line::from(out_spans)
    }
}

pub(crate) struct TranscriptOverlay {
    view: PagerView,
    cells: Vec<Arc<dyn HistoryCell>>,
    highlight_cell: Option<usize>,
    is_done: bool,
}

impl TranscriptOverlay {
    pub(crate) fn new(transcript_cells: Vec<Arc<dyn HistoryCell>>) -> Self {
        let mut view = PagerView::new(
            Self::render_cells_to_texts(&transcript_cells, None),
            "T R A N S C R I P T".to_string(),
            usize::MAX,
        );
        view.cursor_idx = usize::MAX;
        Self {
            view,
            cells: transcript_cells,
            highlight_cell: None,
            is_done: false,
        }
    }

    fn render_cells_to_texts(
        cells: &[Arc<dyn HistoryCell>],
        highlight_cell: Option<usize>,
    ) -> Vec<Text<'static>> {
        let mut texts: Vec<Text<'static>> = Vec::new();
        let mut first = true;
        for (idx, cell) in cells.iter().enumerate() {
            let mut lines: Vec<Line<'static>> = Vec::new();
            if !cell.is_stream_continuation() && !first {
                lines.push(Line::from(""));
            }
            let cell_lines = if Some(idx) == highlight_cell {
                cell.transcript_lines(u16::MAX)
                    .into_iter()
                    .map(Stylize::reversed)
                    .collect()
            } else {
                cell.transcript_lines(u16::MAX)
            };
            lines.extend(cell_lines);
            texts.push(Text::from(lines));
            first = false;
        }
        texts
    }

    pub(crate) fn insert_cell(&mut self, cell: Arc<dyn HistoryCell>) {
        let follow_bottom = self.view.is_scrolled_to_bottom();
        // Append as a new Text chunk (with a separating blank if needed)
        let mut lines: Vec<Line<'static>> = Vec::new();
        if !cell.is_stream_continuation() && !self.cells.is_empty() {
            lines.push(Line::from(""));
        }
        lines.extend(cell.transcript_lines(u16::MAX));
        self.view.texts.push(Text::from(lines));
        self.cells.push(cell);
        self.view.wrap_cache = None;
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    pub(crate) fn set_highlight_cell(&mut self, cell: Option<usize>) {
        self.highlight_cell = cell;
        self.view.wrap_cache = None;
        self.view.texts = Self::render_cells_to_texts(&self.cells, self.highlight_cell);
        if let Some(idx) = self.highlight_cell {
            self.view.scroll_chunk_into_view(idx);
        }
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(line1, buf, PAGER_KEY_HINTS);
        let mut pairs: Vec<(&str, &str)> = vec![("q", "quit"), ("Esc", "edit prev")];
        if self.highlight_cell.is_some() {
            pairs.push(("⏎", "edit message"));
        }
        render_key_hints(line2, buf, &pairs);
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let bottom = Rect::new(area.x, area.y + top_h, area.width, 3);
        self.view.render(top, buf);
        self.render_hints(bottom, buf);
    }
}

impl TranscriptOverlay {
    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => match key_event {
                KeyEvent {
                    code: KeyCode::Char('q'),
                    kind: KeyEventKind::Press,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('t'),
                    modifiers: crossterm::event::KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: crossterm::event::KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                    ..
                } => {
                    // Don't treat plain 'q' as quit when in search input mode; forward to view.
                    if matches!(
                        key_event,
                        KeyEvent {
                            code: KeyCode::Char('q'),
                            ..
                        }
                    ) && self.view.search_input.is_some()
                    {
                        self.view.handle_key_event(tui, key_event)
                    } else {
                        self.is_done = true;
                        Ok(())
                    }
                }
                other => self.view.handle_key_event(tui, other),
            },
            TuiEvent::Draw => {
                tui.draw(u16::MAX, |frame| {
                    self.render(frame.area(), frame.buffer);
                })?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }
}

#[allow(clippy::type_complexity)]
pub(crate) struct StaticOverlay {
    view: PagerView,
    is_done: bool,
    refresh_callback: Option<Box<dyn Fn() -> std::result::Result<Vec<Line<'static>>, String>>>,
    last_refresh_time: Option<Instant>,
    refresh_cooldown: Duration,
}

impl StaticOverlay {
    pub(crate) fn with_title(lines: Vec<Line<'static>>, title: String) -> Self {
        Self {
            view: PagerView::new(vec![Text::from(lines)], title, 0),
            is_done: false,
            refresh_callback: None,
            last_refresh_time: None,
            refresh_cooldown: Duration::from_millis(500),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_title_no_wrap(lines: Vec<Line<'static>>, title: String) -> Self {
        let mut s = Self {
            view: PagerView::new(vec![Text::from(lines)], title, 0),
            is_done: false,
            refresh_callback: None,
            last_refresh_time: None,
            refresh_cooldown: Duration::from_millis(500),
        };
        s.view.wrap_mode = WrapMode::NoWrap;
        s.view.show_cursor_gutter = true;
        s.view.commit_mode = true; // Always start Git Graph in commit navigation mode
        s
    }

    #[allow(dead_code)]
    pub(crate) fn with_title_no_wrap_and_path(
        lines: Vec<Line<'static>>,
        title: String,
        _repo_path: String,
    ) -> Self {
        let mut s = Self {
            view: PagerView::new(vec![Text::from(lines)], title, 0),
            is_done: false,
            refresh_callback: None,
            last_refresh_time: None,
            refresh_cooldown: Duration::from_millis(500),
        };
        s.view.wrap_mode = WrapMode::NoWrap;
        s.view.show_cursor_gutter = true;
        s.view.commit_mode = true;
        s
    }

    pub(crate) fn with_title_no_wrap_refresh(
        lines: Vec<Line<'static>>,
        title: String,
        refresh_callback: Box<dyn Fn() -> std::result::Result<Vec<Line<'static>>, String>>,
    ) -> Self {
        let mut s = Self {
            view: PagerView::new(vec![Text::from(lines)], title, 0),
            is_done: false,
            refresh_callback: Some(refresh_callback),
            last_refresh_time: None,
            refresh_cooldown: Duration::from_millis(500),
        };
        s.view.wrap_mode = WrapMode::NoWrap;
        s.view.show_cursor_gutter = true;
        s.view.commit_mode = true;
        s
    }

    fn can_refresh(&self) -> bool {
        if let Some(last_time) = self.last_refresh_time {
            Instant::now().duration_since(last_time) >= self.refresh_cooldown
        } else {
            true
        }
    }

    fn refresh(&mut self, tui: &mut crate::tui::Tui) {
        // Update the last refresh time
        self.last_refresh_time = Some(Instant::now());

        // First, check if we have a callback and execute it
        let new_content = self.refresh_callback.as_ref().map(|callback| callback());

        // If we have new content to process
        if let Some(result) = new_content {
            // Store current position
            let old_cursor = self.view.cursor_idx;
            let old_scroll = self.view.scroll_offset;

            // Show "Refreshing..." message with cleared content
            let refreshing_text = vec![Text::from(vec![
                Line::from(""),
                Line::from(""),
                Line::from("  Refreshing git graph...".dim()),
            ])];
            self.view.texts = refreshing_text;
            self.view.wrap_cache = None;

            // Force a frame to show the "Refreshing..." message
            let _ = tui.draw(u16::MAX, |frame| {
                self.render(frame.area(), frame.buffer);
            });

            // Sleep briefly to make the message visible
            std::thread::sleep(Duration::from_millis(50));

            // Now actually refresh the content
            match result {
                Ok(new_lines) => {
                    // Update the view with new content
                    self.view.texts = vec![Text::from(new_lines)];
                    self.view.wrap_cache = None; // Force re-wrap

                    // Try to restore position
                    self.view.cursor_idx = old_cursor;
                    self.view.scroll_offset = old_scroll;

                    // Sleep briefly before showing the new content
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    tracing::warn!("Failed to refresh git graph: {}", e);
                    // Show error message briefly
                    let error_text = vec![Text::from(vec![
                        Line::from(""),
                        Line::from(""),
                        Line::from(format!("  Failed to refresh: {e}").red()),
                    ])];
                    self.view.texts = error_text;
                    self.view.wrap_cache = None;
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(line1, buf, PAGER_KEY_HINTS);
        let pairs = if self.refresh_callback.is_some() {
            [("r", "refresh"), ("q", "quit")]
        } else {
            [("q", "quit"), ("", "")]
        };
        render_key_hints(line2, buf, &pairs);
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let bottom = Rect::new(area.x, area.y + top_h, area.width, 3);
        self.view.render(top, buf);
        self.render_hints(bottom, buf);
    }
}

impl StaticOverlay {
    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => match key_event {
                KeyEvent {
                    code: KeyCode::Char('r'),
                    kind: KeyEventKind::Press,
                    ..
                } => {
                    // Refresh the content if callback is available
                    if self.view.search_input.is_none() && self.refresh_callback.is_some() {
                        if self.can_refresh() {
                            self.refresh(tui);
                            tui.frame_requester().schedule_frame();
                        } else {
                            // Cooldown active - ignore silently or could show message
                            tracing::debug!("Refresh on cooldown");
                        }
                    }
                    Ok(())
                }
                KeyEvent {
                    code: KeyCode::Char('q'),
                    kind: KeyEventKind::Press,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: crossterm::event::KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                    ..
                } => {
                    // When search input is active, treat 'q' as input, not quit.
                    if matches!(
                        key_event,
                        KeyEvent {
                            code: KeyCode::Char('q'),
                            ..
                        }
                    ) && self.view.search_input.is_some()
                    {
                        self.view.handle_key_event(tui, key_event)
                    } else {
                        self.is_done = true;
                        Ok(())
                    }
                }
                other => self.view.handle_key_event(tui, other),
            },
            TuiEvent::Draw => {
                tui.draw(u16::MAX, |frame| {
                    self.render(frame.area(), frame.buffer);
                })?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }
}

impl SessionPickerOverlay {
    pub(crate) fn from_state(state: crate::cxresume_picker_widget::PickerState) -> Self {
        let selected_session = state.selected_session().cloned();
        let selected_session_id = selected_session.as_ref().map(|s| s.id.clone());
        Self {
            picker_state: state,
            is_done: false,
            selected_session_id,
            selected_session,
        }
    }

    pub(crate) fn refresh_sessions(&mut self) -> std::result::Result<(), String> {
        let sessions = crate::cxresume_picker_widget::get_cwd_sessions()?;
        self.picker_state.reload_sessions(sessions);
        self.is_done = false;
        self.selected_session_id = None;
        self.selected_session = None;
        Ok(())
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => {
                // Convert KeyCode to PickerEvent using the static method
                if let Some(picker_event) = self.picker_state.key_to_event(key_event.code) {
                    // Handle the picker event and get optional session ID
                    if let Some(session_id) = self.picker_state.handle_event(picker_event) {
                        if session_id.is_empty() {
                            // Empty string signals exit
                            self.selected_session_id = None;
                            self.selected_session = None;
                            self.is_done = true;
                        } else if session_id == crate::cxresume_picker_widget::NEW_SESSION_SENTINEL
                        {
                            self.selected_session_id = Some(session_id);
                            self.selected_session = None;
                            self.is_done = true;
                        } else {
                            self.selected_session_id = Some(session_id);
                            self.selected_session = self.picker_state.selected_session().cloned();
                            self.is_done = true;
                        }
                    }
                    // Schedule a frame update to reflect state changes
                    tui.frame_requester().schedule_frame();
                }
                Ok(())
            }
            TuiEvent::Draw => {
                // Render the picker view
                tui.draw(u16::MAX, |frame| {
                    crate::cxresume_picker_widget::render_picker_view(frame, &self.picker_state);
                })?;
                if self.picker_state.advance_animation() {
                    tui.frame_requester().schedule_frame();
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }

    pub(crate) fn selected_session_info(&self) -> Option<SessionInfo> {
        self.selected_session.clone()
    }

    pub(crate) fn replace_state(&mut self, mut state: crate::cxresume_picker_widget::PickerState) {
        state.inherit_animation(&self.picker_state);
        let selected_session = state.selected_session().cloned();
        let selected_session_id = selected_session.as_ref().map(|s| s.id.clone());
        self.picker_state = state;
        self.selected_session_id = selected_session_id;
        self.selected_session = selected_session;
        self.is_done = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_core::protocol::ExecCommandSource;

    use insta::assert_snapshot;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::exec_cell::CommandOutput;
    use crate::history_cell::HistoryCell;
    use crate::history_cell::new_patch_event;
    use codex_core::protocol::FileChange;
    use codex_protocol::parse_command::ParsedCommand;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[derive(Debug)]
    struct TestCell {
        lines: Vec<Line<'static>>,
    }

    impl crate::history_cell::HistoryCell for TestCell {
        fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
            self.lines.clone()
        }

        fn transcript_lines(&self, _width: u16) -> Vec<Line<'static>> {
            self.lines.clone()
        }
    }

    #[test]
    fn edit_prev_hint_is_visible() {
        let mut overlay = TranscriptOverlay::new(vec![Arc::new(TestCell {
            lines: vec![Line::from("hello")],
        })]);

        // Render into a small buffer and assert the backtrack hint is present
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        // Flatten buffer to a string and check for the hint text
        let mut s = String::new();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                s.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            s.push('\n');
        }
        assert!(
            s.contains("edit prev"),
            "expected 'edit prev' hint in overlay footer, got: {s:?}"
        );
    }

    #[test]
    fn transcript_overlay_snapshot_basic() {
        // Prepare a transcript overlay with a few lines
        let mut overlay = TranscriptOverlay::new(vec![
            Arc::new(TestCell {
                lines: vec![Line::from("alpha")],
            }),
            Arc::new(TestCell {
                lines: vec![Line::from("beta")],
            }),
            Arc::new(TestCell {
                lines: vec![Line::from("gamma")],
            }),
        ]);
        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    fn buffer_to_text(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let symbol = buf[(x, y)].symbol();
                if symbol.is_empty() {
                    out.push(' ');
                } else {
                    out.push(symbol.chars().next().unwrap_or(' '));
                }
            }
            // Trim trailing spaces for stability.
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn transcript_overlay_apply_patch_scroll_vt100_clears_previous_page() {
        let cwd = PathBuf::from("/repo");
        let mut cells: Vec<Arc<dyn HistoryCell>> = Vec::new();

        let mut approval_changes = HashMap::new();
        approval_changes.insert(
            PathBuf::from("foo.txt"),
            FileChange::Add {
                content: "hello\nworld\n".to_string(),
            },
        );
        let approval_cell: Arc<dyn HistoryCell> = Arc::new(new_patch_event(approval_changes, &cwd));
        cells.push(approval_cell);

        let mut apply_changes = HashMap::new();
        apply_changes.insert(
            PathBuf::from("foo.txt"),
            FileChange::Add {
                content: "hello\nworld\n".to_string(),
            },
        );
        let apply_begin_cell: Arc<dyn HistoryCell> = Arc::new(new_patch_event(apply_changes, &cwd));
        cells.push(apply_begin_cell);

        let apply_end_cell: Arc<dyn HistoryCell> =
            Arc::new(crate::history_cell::new_user_approval_decision(vec![
                "✓ Patch applied".green().bold().into(),
                "src/foo.txt".dim().into(),
            ]));
        cells.push(apply_end_cell);

        let mut exec_cell = crate::exec_cell::new_active_exec_command(
            "exec-1".into(),
            vec!["bash".into(), "-lc".into(), "ls".into()],
            vec![ParsedCommand::Unknown { cmd: "ls".into() }],
            ExecCommandSource::Agent,
            None,
            true,
        );
        exec_cell.complete_call(
            "exec-1",
            CommandOutput {
                exit_code: 0,
                aggregated_output: "src\nREADME.md\n".into(),
                formatted_output: "src\nREADME.md\n".into(),
            },
            Duration::from_millis(420),
        );
        let exec_cell: Arc<dyn HistoryCell> = Arc::new(exec_cell);
        cells.push(exec_cell);

        let mut overlay = TranscriptOverlay::new(cells);
        let area = Rect::new(0, 0, 80, 12);
        let mut buf = Buffer::empty(area);

        overlay.render(area, &mut buf);
        overlay.view.scroll_offset = 0;
        overlay.view.wrap_cache = None;
        overlay.render(area, &mut buf);

        let snapshot = buffer_to_text(&buf, area);
        assert_snapshot!("transcript_overlay_apply_patch_scroll_vt100", snapshot);
    }

    #[test]
    fn transcript_overlay_keeps_scroll_pinned_at_bottom() {
        let mut overlay = TranscriptOverlay::new(
            (0..20)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line{i}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("term");
        let initial_offset = overlay.view.scroll_offset;
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");

        let wrapped_len = overlay
            .view
            .wrap_cache
            .as_ref()
            .map(|cache| cache.wrapped.len())
            .unwrap_or(0);
        let height = overlay.view.last_content_height.unwrap_or(0);
        let offset = overlay.view.scroll_offset;
        assert!(
            overlay.view.is_scrolled_to_bottom(),
            "expected initial render to leave view at bottom; initial_offset={initial_offset}, offset={offset}, wrapped_len={wrapped_len}, height={height}"
        );

        overlay.insert_cell(Arc::new(TestCell {
            lines: vec!["tail".into()],
        }));

        assert_eq!(overlay.view.scroll_offset, usize::MAX);
    }

    #[test]
    fn transcript_overlay_preserves_manual_scroll_position() {
        let mut overlay = TranscriptOverlay::new(
            (0..20)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line{i}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");

        overlay.view.scroll_offset = 0;

        overlay.insert_cell(Arc::new(TestCell {
            lines: vec!["tail".into()],
        }));

        assert_eq!(overlay.view.scroll_offset, 0);
    }

    #[test]
    fn static_overlay_snapshot_basic() {
        // Prepare a static overlay with a few lines and a title
        let mut overlay = StaticOverlay::with_title(
            vec!["one".into(), "two".into(), "three".into()],
            "S T A T I C".to_string(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    /// Render transcript overlay and return visible line numbers (`line-NN`) in order.
    fn transcript_line_numbers(overlay: &mut TranscriptOverlay, area: Rect) -> Vec<usize> {
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let content_area = overlay.view.content_area(top);

        let mut nums = Vec::new();
        for y in content_area.y..content_area.bottom() {
            let mut line = String::new();
            for x in content_area.x..content_area.right() {
                line.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            if let Some(n) = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix("line-"))
                .and_then(|s| s.parse().ok())
            {
                nums.push(n);
            }
        }
        nums
    }

    #[test]
    fn transcript_overlay_paging_is_continuous_and_round_trips() {
        let mut overlay = TranscriptOverlay::new(
            (0..50)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line-{i:02}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let area = Rect::new(0, 0, 40, 15);

        // Prime layout so last_content_height is populated and paging uses the real content height.
        let mut buf = Buffer::empty(area);
        overlay.view.scroll_offset = 0;
        overlay.render(area, &mut buf);
        let page_height = overlay.view.page_height(area);

        // Scenario 1: starting from the top, PageDown should show the next page of content.
        overlay.view.scroll_offset = 0;
        let page1 = transcript_line_numbers(&mut overlay, area);
        let page1_len = page1.len();
        let expected_page1: Vec<usize> = (0..page1_len).collect();
        assert_eq!(
            page1, expected_page1,
            "first page should start at line-00 and show a full page of content"
        );

        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let page2 = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            page2.len(),
            page1_len,
            "second page should have the same number of visible lines as the first page"
        );
        let expected_page2_first = *page1.last().unwrap() + 1;
        assert_eq!(
            page2[0], expected_page2_first,
            "second page after PageDown should immediately follow the first page"
        );

        // Scenario 2: from an interior offset (start=3), PageDown then PageUp should round-trip.
        let interior_offset = 3usize;
        overlay.view.scroll_offset = interior_offset;
        let before = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let _ = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_sub(page_height);
        let after = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            before, after,
            "PageDown+PageUp from interior offset ({interior_offset}) should round-trip"
        );

        // Scenario 3: from the top of the second page, PageUp then PageDown should round-trip.
        overlay.view.scroll_offset = page_height;
        let before2 = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_sub(page_height);
        let _ = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let after2 = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            before2, after2,
            "PageUp+PageDown from the top of the second page should round-trip"
        );
    }

    #[test]
    fn pager_wrap_cache_reuses_for_same_width_and_rebuilds_on_change() {
        let long = "This is a long line that should wrap multiple times to ensure non-empty wrapped output.";
        let mut pv = PagerView::new(
            vec![Text::from(vec![long.into()]), Text::from(vec![long.into()])],
            "T".to_string(),
            0,
        );

        // Build cache at width 24
        pv.ensure_wrapped(24);
        let w1 = pv.cached();
        assert!(!w1.is_empty(), "expected wrapped output to be non-empty");
        let ptr1 = w1.as_ptr();

        // Re-run with same width: cache should be reused (pointer stability heuristic)
        pv.ensure_wrapped(24);
        let w2 = pv.cached();
        let ptr2 = w2.as_ptr();
        assert_eq!(ptr1, ptr2, "cache should not rebuild for unchanged width");

        // Change width: cache should rebuild and likely produce different length
        // Drop immutable borrow before mutating
        let prev_len = w2.len();
        pv.ensure_wrapped(36);
        let w3 = pv.cached();
        assert_ne!(
            prev_len,
            w3.len(),
            "wrapped length should change on width change"
        );
    }

    #[test]
    fn pager_wrap_cache_invalidates_on_append() {
        let long = "Another long line for wrapping behavior verification.";
        let mut pv = PagerView::new(vec![Text::from(vec![long.into()])], "T".to_string(), 0);
        pv.ensure_wrapped(28);
        let w1 = pv.cached();
        let len1 = w1.len();

        // Append new lines should cause ensure_wrapped to rebuild due to len change
        pv.texts.push(Text::from(vec![long.into()]));
        pv.texts.push(Text::from(vec![long.into()]));
        pv.ensure_wrapped(28);
        let w2 = pv.cached();
        assert!(
            w2.len() >= len1,
            "wrapped length should grow or stay same after append"
        );
    }
}
