use std::io::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::cxresume_picker_widget::SessionPickerOutcome;
use crate::history_cell::HistoryCell;
use crate::history_cell::UserHistoryCell;
use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::render::Insets;
use crate::render::renderable::InsetRenderable;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;
use crate::tui;
use crate::tui::TuiEvent;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

type GitGraphRefreshCallback =
    Arc<dyn Fn() -> std::result::Result<Vec<Line<'static>>, String> + Send + Sync>;

pub(crate) enum Overlay {
    Transcript(TranscriptOverlay),
    Static(StaticOverlay),
    SessionPicker(Box<SessionPickerOverlay>),
    GitGraph(GitGraphOverlay),
}

/// Session picker overlay integrating PickerState for interactive navigation.
pub(crate) struct SessionPickerOverlay {
    picker_state: crate::cxresume_picker_widget::PickerState,
    codex_home: PathBuf,
    cwd: PathBuf,
    is_done: bool,
    outcome: Option<SessionPickerOutcome>,
}

pub(crate) struct GitGraphOverlay {
    title: String,
    lines: Vec<Line<'static>>,
    is_done: bool,
    scroll_y: usize,
    scroll_x: usize,
    last_content_height: Option<usize>,
    refresh_callback: Option<GitGraphRefreshCallback>,
    refresh_cooldown: Duration,
    last_refresh_time: Option<Instant>,
    refresh_in_flight: bool,
    refresh_rx: Option<mpsc::UnboundedReceiver<std::result::Result<Vec<Line<'static>>, String>>>,
    status_message: Option<(String, Instant)>,
}

impl Overlay {
    pub(crate) fn new_transcript(cells: Vec<Arc<dyn HistoryCell>>) -> Self {
        Self::Transcript(TranscriptOverlay::new(cells))
    }

    pub(crate) fn new_static_with_lines(lines: Vec<Line<'static>>, title: String) -> Self {
        Self::Static(StaticOverlay::with_title(lines, title))
    }

    pub(crate) fn new_static_with_renderables(
        renderables: Vec<Box<dyn Renderable>>,
        title: String,
    ) -> Self {
        Self::Static(StaticOverlay::with_renderables(renderables, title))
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match self {
            Overlay::Transcript(o) => o.handle_event(tui, event),
            Overlay::Static(o) => o.handle_event(tui, event),
            Overlay::SessionPicker(o) => o.handle_event(tui, event),
            Overlay::GitGraph(o) => o.handle_event(tui, event),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Overlay::Transcript(o) => o.is_done(),
            Overlay::Static(o) => o.is_done(),
            Overlay::SessionPicker(o) => o.is_done(),
            Overlay::GitGraph(o) => o.is_done(),
        }
    }

    pub(crate) fn session_picker_outcome(&self) -> Option<SessionPickerOutcome> {
        match self {
            Overlay::SessionPicker(o) => o.outcome(),
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

const KEY_UP: KeyBinding = key_hint::plain(KeyCode::Up);
const KEY_DOWN: KeyBinding = key_hint::plain(KeyCode::Down);
const KEY_K: KeyBinding = key_hint::plain(KeyCode::Char('k'));
const KEY_J: KeyBinding = key_hint::plain(KeyCode::Char('j'));
const KEY_PAGE_UP: KeyBinding = key_hint::plain(KeyCode::PageUp);
const KEY_PAGE_DOWN: KeyBinding = key_hint::plain(KeyCode::PageDown);
const KEY_SPACE: KeyBinding = key_hint::plain(KeyCode::Char(' '));
const KEY_SHIFT_SPACE: KeyBinding = key_hint::shift(KeyCode::Char(' '));
const KEY_HOME: KeyBinding = key_hint::plain(KeyCode::Home);
const KEY_END: KeyBinding = key_hint::plain(KeyCode::End);
const KEY_CTRL_F: KeyBinding = key_hint::ctrl(KeyCode::Char('f'));
const KEY_CTRL_D: KeyBinding = key_hint::ctrl(KeyCode::Char('d'));
const KEY_CTRL_B: KeyBinding = key_hint::ctrl(KeyCode::Char('b'));
const KEY_CTRL_U: KeyBinding = key_hint::ctrl(KeyCode::Char('u'));
const KEY_Q: KeyBinding = key_hint::plain(KeyCode::Char('q'));
const KEY_ESC: KeyBinding = key_hint::plain(KeyCode::Esc);
const KEY_ENTER: KeyBinding = key_hint::plain(KeyCode::Enter);
const KEY_CTRL_T: KeyBinding = key_hint::ctrl(KeyCode::Char('t'));
const KEY_CTRL_C: KeyBinding = key_hint::ctrl(KeyCode::Char('c'));
const KEY_LEFT: KeyBinding = key_hint::plain(KeyCode::Left);
const KEY_RIGHT: KeyBinding = key_hint::plain(KeyCode::Right);
const KEY_H: KeyBinding = key_hint::plain(KeyCode::Char('h'));
const KEY_L: KeyBinding = key_hint::plain(KeyCode::Char('l'));
const KEY_R: KeyBinding = key_hint::plain(KeyCode::Char('r'));

// Common pager navigation hints rendered on the first line
const PAGER_KEY_HINTS: &[(&[KeyBinding], &str)] = &[
    (&[KEY_UP, KEY_DOWN], "to scroll"),
    (&[KEY_PAGE_UP, KEY_PAGE_DOWN], "to page"),
    (&[KEY_HOME, KEY_END], "to jump"),
];

// Render a single line of key hints from (key(s), description) pairs.
fn render_key_hints(area: Rect, buf: &mut Buffer, pairs: &[(&[KeyBinding], &str)]) {
    let mut spans: Vec<Span<'static>> = vec![" ".into()];
    let mut first = true;
    for (keys, desc) in pairs {
        if !first {
            spans.push("   ".into());
        }
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                spans.push("/".into());
            }
            spans.push(Span::from(key));
        }
        spans.push(" ".into());
        spans.push(Span::from(desc.to_string()));
        first = false;
    }
    Paragraph::new(vec![Line::from(spans).dim()]).render_ref(area, buf);
}

impl SessionPickerOverlay {
    pub(crate) fn from_state(
        codex_home: PathBuf,
        cwd: PathBuf,
        mut state: crate::cxresume_picker_widget::PickerState,
    ) -> Self {
        state.reload_aliases(codex_home.as_path());
        Self {
            picker_state: state,
            codex_home,
            cwd,
            is_done: false,
            outcome: None,
        }
    }

    pub(crate) fn refresh_sessions(
        &mut self,
        codex_home: &std::path::Path,
        cwd: &std::path::Path,
    ) -> std::result::Result<(), String> {
        self.codex_home = codex_home.to_path_buf();
        self.cwd = cwd.to_path_buf();
        let state = crate::cxresume_picker_widget::load_picker_state(codex_home, cwd)?;
        self.replace_state(state);
        Ok(())
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => {
                if let Some(picker_event) = self.picker_state.key_to_event(key_event.code) {
                    if let Some(outcome) = self.picker_state.handle_event(picker_event) {
                        if matches!(outcome, SessionPickerOutcome::Refresh) {
                            let codex_home = self.codex_home.clone();
                            let cwd = self.cwd.clone();
                            if let Err(err) =
                                self.refresh_sessions(codex_home.as_path(), cwd.as_path())
                            {
                                tracing::warn!("failed to refresh session picker: {err}");
                            }
                        } else {
                            self.outcome = Some(outcome);
                            self.is_done = true;
                        }
                    }
                    tui.frame_requester().schedule_frame();
                }
                Ok(())
            }
            TuiEvent::Draw => {
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

    pub(crate) fn outcome(&self) -> Option<SessionPickerOutcome> {
        self.outcome.clone()
    }

    pub(crate) fn replace_state(&mut self, mut state: crate::cxresume_picker_widget::PickerState) {
        self.picker_state.reload_aliases(self.codex_home.as_path());
        self.picker_state
            .reload_sessions(std::mem::take(&mut state.sessions));
        self.is_done = false;
        self.outcome = None;
    }
}

impl GitGraphOverlay {
    pub(crate) fn new_with_refresh(
        lines: Vec<Line<'static>>,
        title: String,
        refresh_callback: GitGraphRefreshCallback,
    ) -> Self {
        Self {
            title,
            lines,
            is_done: false,
            scroll_y: 0,
            scroll_x: 0,
            last_content_height: None,
            refresh_callback: Some(refresh_callback),
            refresh_cooldown: Duration::from_millis(500),
            last_refresh_time: None,
            refresh_in_flight: false,
            refresh_rx: None,
            status_message: None,
        }
    }

    fn can_refresh(&self) -> bool {
        if self.refresh_in_flight || self.refresh_callback.is_none() {
            return false;
        }
        self.last_refresh_time.is_none_or(|last_time| {
            Instant::now().duration_since(last_time) >= self.refresh_cooldown
        })
    }

    fn start_refresh(&mut self, frame_requester: crate::tui::FrameRequester) {
        if !self.can_refresh() {
            return;
        }
        let Some(callback) = self.refresh_callback.as_ref() else {
            return;
        };

        self.last_refresh_time = Some(Instant::now());
        self.refresh_in_flight = true;
        self.status_message = Some(("Refreshing git graph…".to_string(), Instant::now()));

        let (tx, rx) = mpsc::unbounded_channel();
        self.refresh_rx = Some(rx);

        let callback = Arc::clone(callback);
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || (callback)()).await;
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.send(outcome);
            frame_requester.schedule_frame();
        });
    }

    fn page_height(&self, viewport_area: Rect) -> usize {
        self.last_content_height
            .unwrap_or_else(|| self.content_area(viewport_area).height as usize)
    }

    fn content_area(&self, area: Rect) -> Rect {
        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        Rect::new(
            top.x,
            top.y.saturating_add(1),
            top.width,
            top.height.saturating_sub(2),
        )
    }

    fn max_line_width(&self) -> usize {
        self.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0)
    }

    fn clamp_scroll(&mut self, content_area: Rect) {
        let visible_h = content_area.height as usize;
        let max_scroll_y = self.lines.len().saturating_sub(visible_h);
        self.scroll_y = if self.scroll_y == usize::MAX {
            max_scroll_y
        } else {
            self.scroll_y.min(max_scroll_y)
        };

        let visible_w = content_area.width as usize;
        let max_scroll_x = self.max_line_width().saturating_sub(visible_w);
        self.scroll_x = self.scroll_x.min(max_scroll_x);
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        Span::from("/ ".repeat(area.width as usize / 2))
            .dim()
            .render_ref(area, buf);
        let header = format!("/ {}", self.title);
        header.dim().render_ref(area, buf);
    }

    fn render_status_bar(&mut self, area: Rect, buf: &mut Buffer, content_area: Rect) {
        if area.height == 0 {
            return;
        }

        let now = Instant::now();
        if let Some((_, created_at)) = self.status_message.as_ref()
            && now.duration_since(*created_at) > Duration::from_secs(2)
        {
            self.status_message = None;
        }

        if area.width == 0 {
            return;
        }

        let sep = "─".repeat(area.width as usize).dim();
        sep.render_ref(area, buf);

        let total = self.lines.len();
        let visible = content_area.height as usize;
        let max_scroll = total.saturating_sub(visible);
        let percent = if max_scroll == 0 {
            100u8
        } else {
            (((self.scroll_y.min(max_scroll)) as f32 / max_scroll as f32) * 100.0).round() as u8
        };

        let left = if total == 0 {
            format!(" y:0/0  x:{} ", self.scroll_x)
        } else {
            format!(
                " y:{}/{}  x:{} ",
                self.scroll_y.saturating_add(1),
                total,
                self.scroll_x
            )
        };

        let mut right = format!(" {percent}% ");
        if self.refresh_in_flight {
            right = format!("{right}refreshing");
        } else if let Some((msg, _)) = &self.status_message {
            right = format!("{right}{msg}");
        }

        let left_w = left.chars().count() as u16;
        let right_w = right.chars().count() as u16;
        let left_area = Rect::new(area.x, area.y, left_w.min(area.width), 1);
        let right_x = area
            .x
            .saturating_add(area.width)
            .saturating_sub(right_w.min(area.width));
        let right_area = Rect::new(right_x, area.y, right_w.min(area.width), 1);
        left.dim().render_ref(left_area, buf);
        right.dim().render_ref(right_area, buf);
    }

    fn render_line_with_horizontal_scroll(
        &self,
        line: &Line<'static>,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let temp_width = self
            .scroll_x
            .saturating_add(area.width as usize)
            .min(u16::MAX as usize) as u16;
        if temp_width == 0 {
            return;
        }
        let mut temp_buf = Buffer::empty(Rect::new(0, 0, temp_width, 1));
        line.render_ref(Rect::new(0, 0, temp_width, 1), &mut temp_buf);

        for x in 0..area.width {
            let src_x = self.scroll_x.saturating_add(x as usize);
            if src_x < temp_width as usize {
                buf[(area.x + x, area.y)] = temp_buf[(src_x as u16, 0)].clone();
            } else {
                buf[(area.x + x, area.y)] = Cell::from(' ');
            }
        }
    }

    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        let mut drawn_bottom = area.y;
        for row in 0..area.height {
            let idx = self.scroll_y.saturating_add(row as usize);
            let row_area = Rect::new(area.x, area.y + row, area.width, 1);
            if let Some(line) = self.lines.get(idx) {
                self.render_line_with_horizontal_scroll(line, row_area, buf);
                drawn_bottom = row_area.bottom();
            } else {
                break;
            }
        }

        for y in drawn_bottom..area.bottom() {
            if area.width == 0 {
                break;
            }
            buf[(area.x, y)] = Cell::from('~');
            for x in area.x + 1..area.right() {
                buf[(x, y)] = Cell::from(' ');
            }
        }
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(
            line1,
            buf,
            &[
                (&[KEY_UP, KEY_DOWN], "to scroll"),
                (&[KEY_LEFT, KEY_RIGHT], "to pan"),
                (&[KEY_PAGE_UP, KEY_PAGE_DOWN], "to page"),
            ],
        );
        let mut pairs: Vec<(&[KeyBinding], &str)> = vec![(&[KEY_Q, KEY_ESC], "to quit")];
        if self.refresh_callback.is_some() {
            pairs.insert(0, (&[KEY_R], "to refresh"));
        }
        render_key_hints(line2, buf, &pairs);
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let bottom = Rect::new(area.x, area.y + top_h, area.width, 3);

        if top.height == 0 {
            return;
        }

        let header = Rect::new(top.x, top.y, top.width, 1);
        self.render_header(header, buf);

        let status = Rect::new(top.x, top.bottom().saturating_sub(1), top.width, 1);
        let content = Rect::new(
            top.x,
            top.y.saturating_add(1),
            top.width,
            top.height.saturating_sub(2),
        );

        self.last_content_height = Some(content.height as usize);
        self.render_content(content, buf);
        self.render_status_bar(status, buf, content);
        self.render_hints(bottom, buf);
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => {
                if KEY_Q.is_press(key_event)
                    || KEY_ESC.is_press(key_event)
                    || KEY_CTRL_C.is_press(key_event)
                {
                    self.is_done = true;
                    return Ok(());
                }

                let page_height = self.page_height(tui.terminal.viewport_area);
                if KEY_UP.is_press(key_event) || KEY_K.is_press(key_event) {
                    self.scroll_y = self.scroll_y.saturating_sub(1);
                } else if KEY_DOWN.is_press(key_event) || KEY_J.is_press(key_event) {
                    self.scroll_y = self.scroll_y.saturating_add(1);
                } else if KEY_PAGE_UP.is_press(key_event) {
                    self.scroll_y = self.scroll_y.saturating_sub(page_height);
                } else if KEY_PAGE_DOWN.is_press(key_event) {
                    self.scroll_y = self.scroll_y.saturating_add(page_height);
                } else if KEY_HOME.is_press(key_event) {
                    self.scroll_y = 0;
                } else if KEY_END.is_press(key_event) {
                    self.scroll_y = usize::MAX;
                } else if KEY_LEFT.is_press(key_event) || KEY_H.is_press(key_event) {
                    self.scroll_x = self.scroll_x.saturating_sub(4);
                } else if KEY_RIGHT.is_press(key_event) || KEY_L.is_press(key_event) {
                    self.scroll_x = self.scroll_x.saturating_add(4);
                } else if KEY_R.is_press(key_event) {
                    self.start_refresh(tui.frame_requester());
                }

                tui.frame_requester().schedule_frame();
                Ok(())
            }
            TuiEvent::Mouse(mouse_event) => {
                let step: usize = 3;
                match mouse_event.kind {
                    MouseEventKind::ScrollUp => {
                        self.scroll_y = self.scroll_y.saturating_sub(step);
                        tui.frame_requester().schedule_frame();
                    }
                    MouseEventKind::ScrollDown => {
                        self.scroll_y = self.scroll_y.saturating_add(step);
                        tui.frame_requester().schedule_frame();
                    }
                    _ => {}
                }
                Ok(())
            }
            TuiEvent::Draw => {
                if self.refresh_in_flight
                    && let Some(rx) = self.refresh_rx.as_mut()
                {
                    match rx.try_recv() {
                        Ok(result) => {
                            self.refresh_in_flight = false;
                            self.refresh_rx = None;
                            match result {
                                Ok(lines) => {
                                    self.lines = lines;
                                    self.status_message =
                                        Some(("Refreshed".to_string(), Instant::now()));
                                }
                                Err(err) => {
                                    self.status_message =
                                        Some((format!("Refresh failed: {err}"), Instant::now()));
                                }
                            }
                        }
                        Err(mpsc::error::TryRecvError::Empty) => {}
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            self.refresh_in_flight = false;
                            self.refresh_rx = None;
                            self.status_message =
                                Some(("Refresh cancelled".to_string(), Instant::now()));
                        }
                    }
                }

                tui.draw(u16::MAX, |frame| {
                    let content_area = self.content_area(frame.area());
                    self.clamp_scroll(content_area);
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

/// Generic widget for rendering a pager view.
struct PagerView {
    renderables: Vec<Box<dyn Renderable>>,
    scroll_offset: usize,
    title: String,
    last_content_height: Option<usize>,
    last_rendered_height: Option<usize>,
    /// If set, on next render ensure this chunk is visible.
    pending_scroll_chunk: Option<usize>,
}

impl PagerView {
    fn new(renderables: Vec<Box<dyn Renderable>>, title: String, scroll_offset: usize) -> Self {
        Self {
            renderables,
            scroll_offset,
            title,
            last_content_height: None,
            last_rendered_height: None,
            pending_scroll_chunk: None,
        }
    }

    fn content_height(&self, width: u16) -> usize {
        self.renderables
            .iter()
            .map(|c| c.desired_height(width) as usize)
            .sum()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        self.render_header(area, buf);
        let content_area = self.content_area(area);
        self.update_last_content_height(content_area.height);
        let content_height = self.content_height(content_area.width);
        self.last_rendered_height = Some(content_height);
        // If there is a pending request to scroll a specific chunk into view,
        // satisfy it now that wrapping is up to date for this width.
        if let Some(idx) = self.pending_scroll_chunk.take() {
            self.ensure_chunk_visible(idx, content_area);
        }
        self.scroll_offset = self
            .scroll_offset
            .min(content_height.saturating_sub(content_area.height as usize));

        self.render_content(content_area, buf);

        self.render_bottom_bar(area, content_area, buf, content_height);
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        Span::from("/ ".repeat(area.width as usize / 2))
            .dim()
            .render_ref(area, buf);
        let header = format!("/ {}", self.title);
        header.dim().render_ref(area, buf);
    }

    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        let mut y = -(self.scroll_offset as isize);
        let mut drawn_bottom = area.y;
        for renderable in &self.renderables {
            let top = y;
            let height = renderable.desired_height(area.width) as isize;
            y += height;
            let bottom = y;
            if bottom < area.y as isize {
                continue;
            }
            if top > area.y as isize + area.height as isize {
                break;
            }
            if top < 0 {
                let drawn = render_offset_content(area, buf, &**renderable, (-top) as u16);
                drawn_bottom = drawn_bottom.max(area.y + drawn);
            } else {
                let draw_height = (height as u16).min(area.height.saturating_sub(top as u16));
                let draw_area = Rect::new(area.x, area.y + top as u16, area.width, draw_height);
                renderable.render(draw_area, buf);
                drawn_bottom = drawn_bottom.max(draw_area.y.saturating_add(draw_area.height));
            }
        }

        for y in drawn_bottom..area.bottom() {
            if area.width == 0 {
                break;
            }
            buf[(area.x, y)] = Cell::from('~');
            for x in area.x + 1..area.right() {
                buf[(x, y)] = Cell::from(' ');
            }
        }
    }

    fn render_bottom_bar(
        &self,
        full_area: Rect,
        content_area: Rect,
        buf: &mut Buffer,
        total_len: usize,
    ) {
        let sep_y = content_area.bottom();
        let sep_rect = Rect::new(full_area.x, sep_y, full_area.width, 1);

        Span::from("─".repeat(sep_rect.width as usize))
            .dim()
            .render_ref(sep_rect, buf);
        let percent = if total_len == 0 {
            100
        } else {
            let max_scroll = total_len.saturating_sub(content_area.height as usize);
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
    }

    fn handle_key_event(&mut self, tui: &mut tui::Tui, key_event: KeyEvent) -> Result<()> {
        match key_event {
            e if KEY_UP.is_press(e) || KEY_K.is_press(e) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            e if KEY_DOWN.is_press(e) || KEY_J.is_press(e) => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            e if KEY_PAGE_UP.is_press(e)
                || KEY_SHIFT_SPACE.is_press(e)
                || KEY_CTRL_B.is_press(e) =>
            {
                let page_height = self.page_height(tui.terminal.viewport_area);
                self.scroll_offset = self.scroll_offset.saturating_sub(page_height);
            }
            e if KEY_PAGE_DOWN.is_press(e) || KEY_SPACE.is_press(e) || KEY_CTRL_F.is_press(e) => {
                let page_height = self.page_height(tui.terminal.viewport_area);
                self.scroll_offset = self.scroll_offset.saturating_add(page_height);
            }
            e if KEY_CTRL_D.is_press(e) => {
                let area = self.content_area(tui.terminal.viewport_area);
                let half_page = (area.height as usize).saturating_add(1) / 2;
                self.scroll_offset = self.scroll_offset.saturating_add(half_page);
            }
            e if KEY_CTRL_U.is_press(e) => {
                let area = self.content_area(tui.terminal.viewport_area);
                let half_page = (area.height as usize).saturating_add(1) / 2;
                self.scroll_offset = self.scroll_offset.saturating_sub(half_page);
            }
            e if KEY_HOME.is_press(e) => {
                self.scroll_offset = 0;
            }
            e if KEY_END.is_press(e) => {
                self.scroll_offset = usize::MAX;
            }
            _ => {
                return Ok(());
            }
        }
        tui.frame_requester()
            .schedule_frame_in(Duration::from_millis(16));
        Ok(())
    }

    fn handle_mouse_scroll(&mut self, tui: &mut tui::Tui, event: MouseEvent) -> Result<()> {
        let step: usize = 3;
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(step);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(step);
            }
            _ => {
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

    fn content_area(&self, area: Rect) -> Rect {
        let mut area = area;
        area.y = area.y.saturating_add(1);
        area.height = area.height.saturating_sub(2);
        area
    }
}

impl PagerView {
    fn is_scrolled_to_bottom(&self) -> bool {
        if self.scroll_offset == usize::MAX {
            return true;
        }
        let Some(height) = self.last_content_height else {
            return false;
        };
        if self.renderables.is_empty() {
            return true;
        }
        let Some(total_height) = self.last_rendered_height else {
            return false;
        };
        if total_height <= height {
            return true;
        }
        let max_scroll = total_height.saturating_sub(height);
        self.scroll_offset >= max_scroll
    }

    /// Request that the given text chunk index be scrolled into view on next render.
    fn scroll_chunk_into_view(&mut self, chunk_index: usize) {
        self.pending_scroll_chunk = Some(chunk_index);
    }

    fn ensure_chunk_visible(&mut self, idx: usize, area: Rect) {
        if area.height == 0 || idx >= self.renderables.len() {
            return;
        }
        let first = self
            .renderables
            .iter()
            .take(idx)
            .map(|r| r.desired_height(area.width) as usize)
            .sum();
        let last = first + self.renderables[idx].desired_height(area.width) as usize;
        let current_top = self.scroll_offset;
        let current_bottom = current_top.saturating_add(area.height.saturating_sub(1) as usize);
        if first < current_top {
            self.scroll_offset = first;
        } else if last > current_bottom {
            self.scroll_offset = last.saturating_sub(area.height.saturating_sub(1) as usize);
        }
    }
}

/// A renderable that caches its desired height.
struct CachedRenderable {
    renderable: Box<dyn Renderable>,
    height: std::cell::Cell<Option<u16>>,
    last_width: std::cell::Cell<Option<u16>>,
}

impl CachedRenderable {
    fn new(renderable: impl Into<Box<dyn Renderable>>) -> Self {
        Self {
            renderable: renderable.into(),
            height: std::cell::Cell::new(None),
            last_width: std::cell::Cell::new(None),
        }
    }
}

impl Renderable for CachedRenderable {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.renderable.render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        if self.last_width.get() != Some(width) {
            let height = self.renderable.desired_height(width);
            self.height.set(Some(height));
            self.last_width.set(Some(width));
        }
        self.height.get().unwrap_or(0)
    }
}

struct CellRenderable {
    cell: Arc<dyn HistoryCell>,
    style: Style,
}

impl Renderable for CellRenderable {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let p =
            Paragraph::new(Text::from(self.cell.transcript_lines(area.width))).style(self.style);
        p.render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.cell.desired_transcript_height(width)
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
        Self {
            view: PagerView::new(
                Self::render_cells(&transcript_cells, None),
                "T R A N S C R I P T".to_string(),
                usize::MAX,
            ),
            cells: transcript_cells,
            highlight_cell: None,
            is_done: false,
        }
    }

    fn render_cells(
        cells: &[Arc<dyn HistoryCell>],
        highlight_cell: Option<usize>,
    ) -> Vec<Box<dyn Renderable>> {
        cells
            .iter()
            .enumerate()
            .flat_map(|(i, c)| {
                let mut v: Vec<Box<dyn Renderable>> = Vec::new();
                let mut cell_renderable = if c.as_any().is::<UserHistoryCell>() {
                    Box::new(CachedRenderable::new(CellRenderable {
                        cell: c.clone(),
                        style: if highlight_cell == Some(i) {
                            user_message_style().reversed()
                        } else {
                            user_message_style()
                        },
                    })) as Box<dyn Renderable>
                } else {
                    Box::new(CachedRenderable::new(CellRenderable {
                        cell: c.clone(),
                        style: Style::default(),
                    })) as Box<dyn Renderable>
                };
                if !c.is_stream_continuation() && i > 0 {
                    cell_renderable = Box::new(InsetRenderable::new(
                        cell_renderable,
                        Insets::tlbr(1, 0, 0, 0),
                    ));
                }
                v.push(cell_renderable);
                v
            })
            .collect()
    }

    pub(crate) fn insert_cell(&mut self, cell: Arc<dyn HistoryCell>) {
        let follow_bottom = self.view.is_scrolled_to_bottom();
        self.cells.push(cell);
        self.view.renderables = Self::render_cells(&self.cells, self.highlight_cell);
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    pub(crate) fn set_highlight_cell(&mut self, cell: Option<usize>) {
        self.highlight_cell = cell;
        self.view.renderables = Self::render_cells(&self.cells, self.highlight_cell);
        if let Some(idx) = self.highlight_cell {
            self.view.scroll_chunk_into_view(idx);
        }
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(line1, buf, PAGER_KEY_HINTS);

        let mut pairs: Vec<(&[KeyBinding], &str)> =
            vec![(&[KEY_Q], "to quit"), (&[KEY_ESC], "to edit prev")];
        if self.highlight_cell.is_some() {
            pairs.push((&[KEY_ENTER], "to edit message"));
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
                e if KEY_Q.is_press(e) || KEY_CTRL_C.is_press(e) || KEY_CTRL_T.is_press(e) => {
                    self.is_done = true;
                    Ok(())
                }
                other => self.view.handle_key_event(tui, other),
            },
            TuiEvent::Mouse(mouse_event) => self.view.handle_mouse_scroll(tui, mouse_event),
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

pub(crate) struct StaticOverlay {
    view: PagerView,
    is_done: bool,
}

impl StaticOverlay {
    pub(crate) fn with_title(lines: Vec<Line<'static>>, title: String) -> Self {
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        Self::with_renderables(vec![Box::new(CachedRenderable::new(paragraph))], title)
    }

    pub(crate) fn with_renderables(renderables: Vec<Box<dyn Renderable>>, title: String) -> Self {
        Self {
            view: PagerView::new(renderables, title, 0),
            is_done: false,
        }
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(line1, buf, PAGER_KEY_HINTS);
        let pairs: Vec<(&[KeyBinding], &str)> = vec![(&[KEY_Q], "to quit")];
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
                e if KEY_Q.is_press(e) || KEY_CTRL_C.is_press(e) => {
                    self.is_done = true;
                    Ok(())
                }
                other => self.view.handle_key_event(tui, other),
            },
            TuiEvent::Mouse(mouse_event) => self.view.handle_mouse_scroll(tui, mouse_event),
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

fn render_offset_content(
    area: Rect,
    buf: &mut Buffer,
    renderable: &dyn Renderable,
    scroll_offset: u16,
) -> u16 {
    let height = renderable.desired_height(area.width);
    let mut tall_buf = Buffer::empty(Rect::new(
        0,
        0,
        area.width,
        height.min(area.height + scroll_offset),
    ));
    renderable.render(*tall_buf.area(), &mut tall_buf);
    let copy_height = area
        .height
        .min(tall_buf.area().height.saturating_sub(scroll_offset));
    for y in 0..copy_height {
        let src_y = y + scroll_offset;
        for x in 0..area.width {
            buf[(area.x + x, area.y + y)] = tall_buf[(x, src_y)].clone();
        }
    }

    copy_height
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_core::protocol::ExecCommandSource;
    use codex_core::protocol::ReviewDecision;
    use insta::assert_snapshot;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::exec_cell::CommandOutput;
    use crate::history_cell;
    use crate::history_cell::HistoryCell;
    use crate::history_cell::new_patch_event;
    use codex_core::protocol::FileChange;
    use codex_protocol::parse_command::ParsedCommand;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::text::Text;

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

    fn paragraph_block(label: &str, lines: usize) -> Box<dyn Renderable> {
        let text = Text::from(
            (0..lines)
                .map(|i| Line::from(format!("{label}{i}")))
                .collect::<Vec<_>>(),
        );
        Box::new(Paragraph::new(text)) as Box<dyn Renderable>
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
            history_cell::new_approval_decision_cell(vec!["ls".into()], ReviewDecision::Approved)
                .into();
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
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");

        assert!(
            overlay.view.is_scrolled_to_bottom(),
            "expected initial render to leave view at bottom"
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
    fn static_overlay_wraps_long_lines() {
        let mut overlay = StaticOverlay::with_title(
            vec!["a very long line that should wrap when rendered within a narrow pager overlay width".into()],
            "S T A T I C".to_string(),
        );
        let mut term = Terminal::new(TestBackend::new(24, 8)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    #[test]
    fn pager_view_content_height_counts_renderables() {
        let pv = PagerView::new(
            vec![paragraph_block("a", 2), paragraph_block("b", 3)],
            "T".to_string(),
            0,
        );

        assert_eq!(pv.content_height(80), 5);
    }

    #[test]
    fn pager_view_ensure_chunk_visible_scrolls_down_when_needed() {
        let mut pv = PagerView::new(
            vec![
                paragraph_block("a", 1),
                paragraph_block("b", 3),
                paragraph_block("c", 3),
            ],
            "T".to_string(),
            0,
        );
        let area = Rect::new(0, 0, 20, 8);

        pv.scroll_offset = 0;
        let content_area = pv.content_area(area);
        pv.ensure_chunk_visible(2, content_area);

        let mut buf = Buffer::empty(area);
        pv.render(area, &mut buf);
        let rendered = buffer_to_text(&buf, area);

        assert!(
            rendered.contains("c0"),
            "expected chunk top in view: {rendered:?}"
        );
        assert!(
            rendered.contains("c1"),
            "expected chunk middle in view: {rendered:?}"
        );
        assert!(
            rendered.contains("c2"),
            "expected chunk bottom in view: {rendered:?}"
        );
    }

    #[test]
    fn pager_view_ensure_chunk_visible_scrolls_up_when_needed() {
        let mut pv = PagerView::new(
            vec![
                paragraph_block("a", 2),
                paragraph_block("b", 3),
                paragraph_block("c", 3),
            ],
            "T".to_string(),
            0,
        );
        let area = Rect::new(0, 0, 20, 3);

        pv.scroll_offset = 6;
        pv.ensure_chunk_visible(0, area);

        assert_eq!(pv.scroll_offset, 0);
    }

    #[test]
    fn pager_view_is_scrolled_to_bottom_accounts_for_wrapped_height() {
        let mut pv = PagerView::new(vec![paragraph_block("a", 10)], "T".to_string(), 0);
        let area = Rect::new(0, 0, 20, 8);
        let mut buf = Buffer::empty(area);

        pv.render(area, &mut buf);

        assert!(
            !pv.is_scrolled_to_bottom(),
            "expected view to report not at bottom when offset < max"
        );

        pv.scroll_offset = usize::MAX;
        pv.render(area, &mut buf);

        assert!(
            pv.is_scrolled_to_bottom(),
            "expected view to report at bottom after scrolling to end"
        );
    }
}
