use crate::pager_overlay::Overlay;
use crate::render::line_utils;
use codex_ansi_escape::ansi_escape_line;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Paragraph;
use serde::Deserialize;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use tracing::warn;

#[cfg(not(target_os = "android"))]
use arboard::Clipboard;

pub const NEW_SESSION_SENTINEL: &str = "__cxresume_new_session__";
const FULL_PREVIEW_WRAP_WIDTH: usize = 76;
const THEME_GRAY: Color = Color::Rgb(0x80, 0x80, 0x80);
const THEME_GREEN: Color = Color::Rgb(0x5a, 0xf7, 0x8e);
const THEME_YELLOW: Color = Color::Rgb(0xf3, 0xf9, 0x9d);
const THEME_ORANGE: Color = Color::Rgb(0xff, 0x95, 0x00);
const THEME_PURPLE: Color = Color::Rgb(0xbf, 0x5a, 0xf2);
const THEME_CYAN: Color = Color::Rgb(0x5f, 0xbe, 0xaa);
const THEME_BLUE: Color = Color::Rgb(0x6a, 0xc8, 0xff);
const THEME_PINK: Color = Color::Rgb(0xff, 0x6a, 0xc1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TumixState {
    Running,
    Completed,
    Failed,
    Stalled,
}

fn last_role_color(role: &str) -> Color {
    match role {
        "Assistant" => THEME_GREEN,
        "User" => THEME_PINK,
        _ => THEME_GRAY,
    }
}

fn dialog_role_color(role: &str) -> Color {
    match role {
        "User" => THEME_ORANGE,
        "Assistant" => THEME_GREEN,
        _ => THEME_GRAY,
    }
}

fn stylize_session_id(id: &str) -> String {
    format!("{}", id.fg(THEME_ORANGE).bold())
}

fn session_id_span(id: &str) -> Span<'static> {
    Span::styled(
        id.to_string(),
        Style::default()
            .fg(THEME_ORANGE)
            .add_modifier(Modifier::BOLD),
    )
}

fn session_age_span(age: &str) -> Span<'static> {
    Span::styled(format!("({age})"), Style::default().fg(THEME_GRAY))
}

fn stylize_model_name(model: &str) -> String {
    model.fg(THEME_BLUE).to_string()
}

fn stylize_last_role_text(role: &str) -> String {
    role.fg(last_role_color(role)).to_string()
}

fn stylize_label(label: &str) -> String {
    label.fg(THEME_GRAY).to_string()
}

fn stylize_messages_count(count: usize) -> String {
    count.to_string().fg(THEME_YELLOW).to_string()
}

fn stylize_separator() -> String {
    " • ".fg(THEME_GRAY).to_string()
}

fn stylize_cwd(cwd: &str) -> String {
    if cwd.is_empty() {
        return "-".fg(THEME_GRAY).to_string();
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let display = if !home.is_empty() && cwd.starts_with(&home) {
        cwd.replacen(&home, "~", 1)
    } else {
        cwd.to_string()
    };

    if let Some(rest) = display.strip_prefix("~/") {
        format!("{}{}", "~/".fg(THEME_PURPLE), rest.fg(THEME_CYAN))
    } else if display == "~" {
        "~".fg(THEME_PURPLE).to_string()
    } else {
        display.fg(THEME_CYAN).to_string()
    }
}

fn colored_bar_span(color: Color) -> Span<'static> {
    Span::styled("┃ ".to_string(), Style::default().fg(color))
}

fn tumix_state_color(state: TumixState) -> Color {
    match state {
        TumixState::Running => THEME_YELLOW,
        TumixState::Completed => THEME_GREEN,
        TumixState::Failed => THEME_PINK,
        TumixState::Stalled => THEME_PURPLE,
    }
}

fn tumix_badge_span() -> Span<'static> {
    Span::styled(
        "[Tumix]".to_string(),
        Style::default()
            .fg(THEME_PURPLE)
            .add_modifier(Modifier::BOLD),
    )
}

fn neutral_indicator_span() -> Span<'static> {
    Span::styled("○".to_string(), Style::default().fg(THEME_GRAY))
}

fn tumix_indicator_span(session: &SessionInfo, frame: usize) -> Span<'static> {
    if let Some(indicator) = session.tumix.as_ref() {
        let color = tumix_state_color(indicator.state);
        let base_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
        match indicator.state {
            TumixState::Running => {
                let frames = ["◐", "◓", "◑", "◒"];
                Span::styled(frames[frame % frames.len()].to_string(), base_style)
            }
            _ => Span::styled("●".to_string(), base_style),
        }
    } else {
        neutral_indicator_span()
    }
}

#[derive(Debug, Clone)]
pub struct TumixIndicator {
    pub run_id: String,
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub branch: Option<String>,
    pub state: TumixState,
    pub error: Option<String>,
}

/// Enhanced session metadata with comprehensive information
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: String,
    pub path: PathBuf,
    pub cwd: String,
    pub age: String,
    pub mtime: u64,
    pub message_count: usize,
    pub last_role: String,
    #[allow(dead_code)]
    pub total_tokens: usize,
    pub model: String,
    pub tumix: Option<TumixIndicator>,
}

#[derive(Default)]
pub struct TumixStatusIndex {
    by_session: HashMap<String, TimedIndicator>,
    by_path: HashMap<PathBuf, TimedIndicator>,
}

#[derive(Clone)]
struct TimedIndicator {
    indicator: TumixIndicator,
    modified: Option<SystemTime>,
}

impl TumixStatusIndex {
    fn insert(
        &mut self,
        session_id: &str,
        path: Option<PathBuf>,
        indicator: TumixIndicator,
        modified: Option<SystemTime>,
    ) {
        let entry = TimedIndicator {
            indicator,
            modified,
        };
        self.upsert_session(session_id, entry.clone());
        if let Some(path) = path {
            self.upsert_path(path, entry);
        }
    }

    fn upsert_session(&mut self, session_id: &str, entry: TimedIndicator) {
        match self.by_session.get_mut(session_id) {
            Some(existing) => {
                if should_replace(existing, &entry) {
                    *existing = entry;
                }
            }
            None => {
                self.by_session.insert(session_id.to_string(), entry);
            }
        }
    }

    fn upsert_path(&mut self, path: PathBuf, entry: TimedIndicator) {
        match self.by_path.get_mut(&path) {
            Some(existing) => {
                if should_replace(existing, &entry) {
                    *existing = entry;
                }
            }
            None => {
                self.by_path.insert(path, entry);
            }
        }
    }

    pub fn lookup(&self, session_id: &str, path: &Path) -> Option<TumixIndicator> {
        if let Some(entry) = self.by_session.get(session_id) {
            return Some(entry.indicator.clone());
        }

        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(entry) = self.by_path.get(&canonical) {
            return Some(entry.indicator.clone());
        }
        self.by_path.get(path).map(|entry| entry.indicator.clone())
    }
}

fn should_replace(current: &TimedIndicator, candidate: &TimedIndicator) -> bool {
    let current_rank = state_rank(current.indicator.state);
    let candidate_rank = state_rank(candidate.indicator.state);

    match (current.modified, candidate.modified) {
        (Some(cur), Some(new)) => new > cur,
        (None, Some(_)) => true,
        (Some(_), None) => candidate_rank > current_rank,
        (None, None) => candidate_rank > current_rank,
    }
}

fn state_rank(state: TumixState) -> u8 {
    match state {
        TumixState::Completed => 4,
        TumixState::Failed => 3,
        TumixState::Running => 2,
        TumixState::Stalled => 1,
    }
}

/// Cached preview data for a session (messages and metadata)
#[derive(Debug, Clone)]
struct PreviewCache {
    #[allow(dead_code)]
    messages: Vec<(String, String, String)>, // (role, content, timestamp)
    #[allow(dead_code)]
    cached_at: u64, // Unix timestamp when cached
}

/// Message summary for quick access (count and last role)
#[derive(Debug, Clone)]
pub struct MessageSummary {
    #[allow(dead_code)]
    message_count: usize,
    #[allow(dead_code)]
    last_role: String,
    #[allow(dead_code)]
    last_update: u64,
}

/// Multi-layered cache for session picker performance
/// Stores metadata, previews, and message summaries to avoid repeated file I/O
#[derive(Debug, Clone)]
pub struct CacheLayer {
    // Session metadata cache (keyed by file path)
    #[allow(dead_code)]
    meta_cache: HashMap<PathBuf, SessionInfo>,

    // Preview cache (keyed by session ID) - stores formatted message previews
    preview_cache: HashMap<String, PreviewCache>,

    // Message summary cache (keyed by file path) - lightweight alternative to full preview
    #[allow(dead_code)]
    summary_cache: HashMap<PathBuf, MessageSummary>,

    // Cache hit/miss statistics
    #[allow(dead_code)]
    meta_hits: usize,
    #[allow(dead_code)]
    meta_misses: usize,
    #[allow(dead_code)]
    preview_hits: usize,
    #[allow(dead_code)]
    preview_misses: usize,
}

impl CacheLayer {
    /// Create a new empty cache layer
    pub fn new() -> Self {
        CacheLayer {
            meta_cache: HashMap::new(),
            preview_cache: HashMap::new(),
            summary_cache: HashMap::new(),
            meta_hits: 0,
            meta_misses: 0,
            preview_hits: 0,
            preview_misses: 0,
        }
    }

    /// Get or insert session metadata in cache
    #[allow(dead_code)]
    pub fn get_or_insert_meta(&mut self, path: &PathBuf, default: SessionInfo) -> SessionInfo {
        if self.meta_cache.contains_key(path) {
            self.meta_hits += 1;
            self.meta_cache[path].clone()
        } else {
            self.meta_misses += 1;
            self.meta_cache.insert(path.clone(), default.clone());
            default
        }
    }

    /// Get preview from cache if available
    pub fn get_preview(&mut self, session_id: &str) -> Option<Vec<(String, String, String)>> {
        if let Some(cached) = self.preview_cache.get(session_id) {
            self.preview_hits += 1;
            Some(cached.messages.clone())
        } else {
            self.preview_misses += 1;
            None
        }
    }

    /// Store preview in cache
    pub fn cache_preview(&mut self, session_id: String, messages: Vec<(String, String, String)>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.preview_cache.insert(
            session_id,
            PreviewCache {
                messages,
                cached_at: now,
            },
        );
    }

    /// Get message summary from cache if available
    #[allow(dead_code)]
    pub fn get_summary(&mut self, path: &PathBuf) -> Option<MessageSummary> {
        self.summary_cache.get(path).cloned()
    }

    /// Store message summary in cache
    #[allow(dead_code)]
    pub fn cache_summary(&mut self, path: PathBuf, message_count: usize, last_role: String) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.summary_cache.insert(
            path,
            MessageSummary {
                message_count,
                last_role,
                last_update: now,
            },
        );
    }

    /// Remove a specific preview from cache
    pub fn remove_preview(&mut self, session_id: &str) {
        self.preview_cache.remove(session_id);
    }

    /// Clear all caches (useful for refresh operations)
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.meta_cache.clear();
        self.preview_cache.clear();
        self.summary_cache.clear();
    }

    /// Get cache statistics for debugging
    #[allow(dead_code)]
    pub fn stats(&self) -> (usize, usize, usize, usize) {
        (
            self.meta_hits,
            self.meta_misses,
            self.preview_hits,
            self.preview_misses,
        )
    }
}

impl Default for CacheLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// Split View Layout Manager for dual-panel picker
#[derive(Debug, Clone)]
pub struct SplitLayout {
    pub left_width: u16,  // Left panel width (35%)
    pub right_width: u16, // Right panel width (65%)
    #[allow(dead_code)]
    pub total_height: u16,
    #[allow(dead_code)]
    pub total_width: u16,
    #[allow(dead_code)]
    pub gap: u16, // Space between panels
}

impl SplitLayout {
    /// Create a new split layout from total dimensions
    pub fn new(total_width: u16, total_height: u16) -> Self {
        // Account for 1-char gap between panels
        let usable_width = total_width.saturating_sub(1);

        // 35% left, 65% right
        let left_width = (usable_width as f32 * 0.35) as u16;
        let right_width = usable_width.saturating_sub(left_width);

        SplitLayout {
            left_width,
            right_width,
            total_height,
            total_width,
            gap: 1,
        }
    }

    /// Get the left panel area (0, 0, left_width, total_height)
    #[allow(dead_code)]
    pub fn left_area(&self) -> (u16, u16, u16, u16) {
        (0, 0, self.left_width, self.total_height)
    }

    /// Get the right panel area
    #[allow(dead_code)]
    pub fn right_area(&self) -> (u16, u16, u16, u16) {
        let x = self.left_width + self.gap;
        (x, 0, self.right_width, self.total_height)
    }
}

/// View mode for the picker
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Split,       // Dual-panel view (default)
    FullPreview, // Full-screen message preview
    SessionOnly, // Full-screen session list
}

/// Which pane currently has keyboard focus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    LeftList,
    RightPreview,
}

/// Pagination manager for session lists
#[derive(Debug, Clone)]
pub struct Pagination {
    pub total_items: usize,
    pub items_per_page: usize,
    pub current_page: usize,
}

impl Pagination {
    /// Create new pagination from total items
    pub fn new(total_items: usize, items_per_page: usize) -> Self {
        Pagination {
            total_items,
            items_per_page,
            current_page: 0,
        }
    }

    /// Get the total number of pages
    pub fn total_pages(&self) -> usize {
        self.total_items.div_ceil(self.items_per_page)
    }

    /// Get the start index for current page
    pub fn page_start(&self) -> usize {
        self.current_page * self.items_per_page
    }

    /// Get the end index for current page (exclusive)
    pub fn page_end(&self) -> usize {
        ((self.current_page + 1) * self.items_per_page).min(self.total_items)
    }

    /// Get items for the current page (slice indices)
    pub fn page_range(&self) -> std::ops::Range<usize> {
        self.page_start()..self.page_end()
    }

    /// Move to next page
    pub fn next_page(&mut self) -> bool {
        if self.current_page + 1 < self.total_pages() {
            self.current_page += 1;
            true
        } else {
            false
        }
    }

    /// Move to previous page
    pub fn prev_page(&mut self) -> bool {
        if self.current_page > 0 {
            self.current_page -= 1;
            true
        } else {
            false
        }
    }

    /// Jump to first page
    #[allow(dead_code)]
    pub fn first_page(&mut self) {
        self.current_page = 0;
    }

    /// Jump to last page
    #[allow(dead_code)]
    pub fn last_page(&mut self) {
        self.current_page = self.total_pages().saturating_sub(1);
    }

    /// Check if there's a next page
    #[allow(dead_code)]
    pub fn has_next(&self) -> bool {
        self.current_page + 1 < self.total_pages()
    }

    /// Check if there's a previous page
    #[allow(dead_code)]
    pub fn has_prev(&self) -> bool {
        self.current_page > 0
    }
}

/// State management for the session picker
#[derive(Debug, Clone)]
pub struct PickerState {
    pub sessions: Vec<SessionInfo>,
    pub selected_idx: usize,
    #[allow(dead_code)]
    pub scroll_offset_left: usize, // For left panel scrolling
    pub scroll_offset_right: usize, // For right panel scrolling
    pub pagination: Pagination,     // Pagination manager
    pub view_mode: ViewMode,
    pub focus: FocusPane,
    pub modal_active: bool,    // Delete or edit confirmation dialog
    pub modal_message: String, // Message to display in modal
    pub cache: CacheLayer,     // Multi-layered cache for performance
    animation_tick: usize,
    has_running_tumix: bool,
}

impl PickerState {
    /// Create a new picker state from sessions list
    pub fn new(sessions: Vec<SessionInfo>) -> Self {
        let items_count = sessions.len();
        let pagination = Pagination::new(items_count, 30); // 30 items per page
        let running = has_running_tumix(&sessions);
        let mut picker_state = PickerState {
            sessions,
            selected_idx: 0,
            scroll_offset_left: 0,
            scroll_offset_right: 0,
            pagination,
            view_mode: ViewMode::Split,
            focus: FocusPane::LeftList,
            modal_active: false,
            modal_message: String::new(),
            cache: CacheLayer::new(),
            animation_tick: 0,
            has_running_tumix: running,
        };
        // Prefetch the first visible page of sessions on initial load
        picker_state.prefetch_visible_page();
        picker_state
    }

    /// Get currently selected session
    pub fn selected_session(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.selected_idx)
    }

    /// Get sessions for the current page
    #[allow(dead_code)]
    pub fn current_page_sessions(&self) -> &[SessionInfo] {
        let range = self.pagination.page_range();
        &self.sessions[range]
    }

    /// Move to next page and reset selection to first item on page
    pub fn next_page(&mut self) {
        if self.pagination.next_page() {
            self.selected_idx = self.pagination.page_start();
            self.scroll_offset_right = 0;
            // Prefetch visible page for faster display
            self.prefetch_visible_page();
        }
    }

    /// Move to previous page and reset selection to first item on page
    pub fn prev_page(&mut self) {
        if self.pagination.prev_page() {
            self.selected_idx = self.pagination.page_start();
            self.scroll_offset_right = 0;
            // Prefetch visible page for faster display
            self.prefetch_visible_page();
        }
    }

    /// Move selection up
    pub fn select_prev(&mut self) {
        if self.selected_idx > 0 {
            self.selected_idx -= 1;
            self.scroll_offset_right = 0; // Reset preview scroll
            // Prefetch adjacent sessions for smooth navigation
            self.prefetch_adjacent_sessions();
        }
    }

    /// Move selection down
    pub fn select_next(&mut self) {
        if self.selected_idx < self.sessions.len().saturating_sub(1) {
            self.selected_idx += 1;
            self.scroll_offset_right = 0; // Reset preview scroll
            // Prefetch adjacent sessions for smooth navigation
            self.prefetch_adjacent_sessions();
        }
    }

    /// Jump to first session
    pub fn select_first(&mut self) {
        self.selected_idx = 0;
        self.scroll_offset_right = 0;
    }

    /// Jump to last session
    pub fn select_last(&mut self) {
        self.selected_idx = self.sessions.len().saturating_sub(1);
        self.scroll_offset_right = 0;
    }

    /// Scroll preview up
    pub fn scroll_preview_up(&mut self) {
        self.scroll_offset_right = self.scroll_offset_right.saturating_sub(1);
    }

    /// Scroll preview down
    pub fn scroll_preview_down(&mut self) {
        self.scroll_offset_right = self.scroll_offset_right.saturating_add(1);
    }

    /// Toggle view mode
    pub fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Split => ViewMode::FullPreview,
            ViewMode::FullPreview => ViewMode::SessionOnly,
            ViewMode::SessionOnly => ViewMode::Split,
        };

        match self.view_mode {
            ViewMode::Split => self.focus_left(),
            ViewMode::FullPreview => {
                self.focus_right();
                self.scroll_offset_right = 0;
            }
            ViewMode::SessionOnly => self.focus_left(),
        }
    }

    /// Open delete confirmation modal
    pub fn confirm_delete(&mut self) {
        if let Some(session) = self.sessions.get(self.selected_idx) {
            self.modal_active = true;
            self.modal_message = format!(
                "Delete session '{}'?\nThis action cannot be undone.\n\nPress 'y' to confirm or 'n' to cancel.",
                session.id
            );
        }
    }

    /// Close any active modal
    pub fn close_modal(&mut self) {
        self.modal_active = false;
        self.modal_message.clear();
    }

    /// Get cached preview or fetch it from file
    #[allow(dead_code)]
    pub fn get_or_fetch_preview(
        &mut self,
        session: &SessionInfo,
        limit: usize,
    ) -> Vec<(String, String, String)> {
        // Try to get from cache first
        if let Some(cached) = self.cache.get_preview(&session.id) {
            return cached;
        }

        // Not in cache, extract from file and cache it
        let messages = extract_recent_messages_with_timestamps(&session.path, limit);
        self.cache
            .cache_preview(session.id.clone(), messages.clone());
        messages
    }

    /// Get cache statistics
    #[allow(dead_code)]
    pub fn cache_stats(&self) -> (usize, usize, usize, usize) {
        self.cache.stats()
    }

    /// Clear the entire cache (for refresh operations)
    #[allow(dead_code)]
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Prefetch preview for session at index (non-blocking optimization)
    /// This method loads the preview into cache if not already cached
    pub fn prefetch_preview_for_index(&mut self, idx: usize) {
        if let Some(session) = self.sessions.get(idx) {
            // Only prefetch if not already in cache
            if self.cache.get_preview(&session.id).is_none() {
                let messages = extract_recent_messages_with_timestamps(&session.path, 6);
                self.cache.cache_preview(session.id.clone(), messages);
            }
        }
    }

    /// Prefetch adjacent sessions (previous and next) when navigating
    /// This provides lazy loading benefit without blocking the UI
    pub fn prefetch_adjacent_sessions(&mut self) {
        // Prefetch next session
        if self.selected_idx + 1 < self.sessions.len() {
            self.prefetch_preview_for_index(self.selected_idx + 1);
        }

        // Prefetch previous session
        if self.selected_idx > 0 {
            self.prefetch_preview_for_index(self.selected_idx - 1);
        }
    }

    /// Prefetch visible page of sessions for immediate display
    /// Useful when paginating or view first loads
    pub fn prefetch_visible_page(&mut self) {
        let range = self.pagination.page_range();
        for idx in range {
            self.prefetch_preview_for_index(idx);
        }
    }

    pub fn reload_sessions(&mut self, sessions: Vec<SessionInfo>) {
        let per_page = self.pagination.items_per_page;
        let previous_id = self.selected_session().map(|s| s.id.clone());

        self.sessions = sessions;
        self.cache.clear();
        self.pagination = Pagination::new(self.sessions.len(), per_page);

        if self.sessions.is_empty() {
            self.selected_idx = 0;
            self.scroll_offset_right = 0;
            self.refresh_running_flag();
            return;
        }

        let mut selected_idx = previous_id
            .and_then(|id| self.sessions.iter().position(|s| s.id == id))
            .unwrap_or(0);
        if selected_idx >= self.sessions.len() {
            selected_idx = 0;
        }

        self.selected_idx = selected_idx;
        self.pagination.current_page = selected_idx / per_page.max(1);
        self.scroll_offset_right = 0;

        self.prefetch_visible_page();
        self.prefetch_adjacent_sessions();
        self.refresh_running_flag();
    }

    fn focus_left(&mut self) {
        if matches!(self.view_mode, ViewMode::Split | ViewMode::SessionOnly) {
            self.focus = FocusPane::LeftList;
        }
    }

    fn focus_right(&mut self) {
        if matches!(self.view_mode, ViewMode::Split | ViewMode::FullPreview) {
            self.focus = FocusPane::RightPreview;
        }
    }

    pub fn advance_animation(&mut self) -> bool {
        if self.has_running_tumix {
            self.animation_tick = self.animation_tick.wrapping_add(1);
            true
        } else {
            false
        }
    }

    pub fn animation_frame(&self) -> usize {
        self.animation_tick
    }

    fn refresh_running_flag(&mut self) {
        self.has_running_tumix = has_running_tumix(&self.sessions);
        if !self.has_running_tumix {
            self.animation_tick = 0;
        }
    }

    pub fn has_running_tumix(&self) -> bool {
        self.has_running_tumix
    }

    pub fn inherit_animation(&mut self, other: &PickerState) {
        if self.has_running_tumix && other.has_running_tumix() {
            self.animation_tick = other.animation_frame();
        }
    }
}

/// Event type enum for picker keyboard input
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerEvent {
    // Navigation
    SelectNext,
    SelectPrev,
    SelectFirst,
    SelectLast,
    PageNext,
    PagePrev,

    // Preview scrolling
    #[allow(dead_code)]
    ScrollUp,
    #[allow(dead_code)]
    ScrollDown,
    FocusLeft,
    FocusRight,

    // Actions
    Resume, // Enter key - return selected session
    Delete, // d key - confirm delete
    #[allow(dead_code)]
    ToggleViewMode, // f key - cycle through views
    CopySessionId, // c key - copy to clipboard
    NewSession, // n key - create new

    // Navigation modes
    CycleViewMode, // f key - Split → FullPreview → SessionOnly → Split
    Refresh,       // r key - refresh sessions list

    // Dialog control
    ConfirmAction, // y key in modal
    #[allow(dead_code)]
    CancelAction, // n key in modal

    // Exit
    Exit, // q or Esc
}

impl PickerState {
    /// Handle a picker event and update state accordingly
    /// Returns Some(session_id) when: (1) session to resume, (2) session to delete (empty string = exit)
    pub fn handle_event(&mut self, event: PickerEvent) -> Option<String> {
        if self.modal_active {
            // In modal mode, only handle confirm/cancel
            match event {
                PickerEvent::ConfirmAction => {
                    // Confirm delete: remove session file and from list
                    if let Some(session) = self.sessions.get(self.selected_idx) {
                        let session_id = session.id.clone();
                        let session_path = session.path.clone();
                        self.modal_active = false;

                        // Remove from sessions list
                        self.sessions.remove(self.selected_idx);

                        // Adjust selected index if needed
                        if self.selected_idx >= self.sessions.len() && self.selected_idx > 0 {
                            self.selected_idx -= 1;
                        }

                        // Update pagination total
                        self.pagination.total_items = self.sessions.len();

                        // Delete the file
                        let _ = fs::remove_file(&session_path);

                        // Clear cache entries for this session
                        self.cache.remove_preview(&session_id);
                    }
                }
                PickerEvent::CancelAction => {
                    self.close_modal();
                }
                _ => {}
            }
            return None;
        }

        // Normal mode event handling
        match event {
            PickerEvent::FocusLeft => self.focus_left(),
            PickerEvent::FocusRight => self.focus_right(),

            PickerEvent::SelectNext => {
                if self.focus == FocusPane::LeftList {
                    self.select_next();
                } else {
                    self.scroll_preview_down();
                }
            }
            PickerEvent::SelectPrev => {
                if self.focus == FocusPane::LeftList {
                    self.select_prev();
                } else {
                    self.scroll_preview_up();
                }
            }
            PickerEvent::SelectFirst => {
                if self.focus == FocusPane::LeftList {
                    self.select_first();
                } else {
                    self.scroll_offset_right = 0;
                }
            }
            PickerEvent::SelectLast => {
                if self.focus == FocusPane::LeftList {
                    self.select_last();
                } else {
                    self.scroll_offset_right = usize::MAX;
                }
            }
            PickerEvent::PageNext => {
                if self.focus == FocusPane::LeftList {
                    self.next_page();
                } else {
                    self.scroll_offset_right = self.scroll_offset_right.saturating_add(10);
                }
            }
            PickerEvent::PagePrev => {
                if self.focus == FocusPane::LeftList {
                    self.prev_page();
                } else {
                    self.scroll_offset_right = self.scroll_offset_right.saturating_sub(10);
                }
            }

            PickerEvent::ScrollUp => {
                self.focus_right();
                if self.focus == FocusPane::RightPreview {
                    self.scroll_preview_up();
                }
            }
            PickerEvent::ScrollDown => {
                self.focus_right();
                if self.focus == FocusPane::RightPreview {
                    self.scroll_preview_down();
                }
            }

            PickerEvent::ToggleViewMode | PickerEvent::CycleViewMode => {
                self.toggle_view_mode();
            }

            PickerEvent::Resume => {
                if let Some(session) = self.selected_session() {
                    return Some(session.id.clone());
                }
            }

            PickerEvent::Delete => {
                self.confirm_delete();
            }

            PickerEvent::CopySessionId => {
                if let Some(session) = self.selected_session()
                    && let Err(err) = copy_to_clipboard(session.id.as_str())
                {
                    warn!("failed to copy session id: {err}");
                }
            }

            PickerEvent::NewSession => {
                return Some(NEW_SESSION_SENTINEL.to_string());
            }

            PickerEvent::Refresh => {
                // Would trigger refresh callback
            }

            PickerEvent::Exit => {
                return Some(String::new()); // Signal exit
            }

            _ => {}
        }

        None
    }

    /// Convert KeyEvent to PickerEvent (for integration with Overlay)
    pub fn key_to_event(&self, key_code: crossterm::event::KeyCode) -> Option<PickerEvent> {
        use crossterm::event::KeyCode;

        if self.modal_active {
            return match key_code {
                KeyCode::Char('y') | KeyCode::Enter => Some(PickerEvent::ConfirmAction),
                KeyCode::Char('n') | KeyCode::Esc => Some(PickerEvent::CancelAction),
                _ => None,
            };
        }

        match key_code {
            KeyCode::Up => Some(PickerEvent::SelectPrev),
            KeyCode::Down => Some(PickerEvent::SelectNext),
            KeyCode::Home => Some(PickerEvent::SelectFirst),
            KeyCode::End => Some(PickerEvent::SelectLast),
            KeyCode::PageUp => Some(PickerEvent::PagePrev),
            KeyCode::PageDown => Some(PickerEvent::PageNext),
            KeyCode::Left => Some(PickerEvent::FocusLeft),
            KeyCode::Right => Some(PickerEvent::FocusRight),

            KeyCode::Enter => Some(PickerEvent::Resume),
            KeyCode::Char('d') => Some(PickerEvent::Delete),
            KeyCode::Char('f') => Some(PickerEvent::CycleViewMode),
            KeyCode::Char('c') => Some(PickerEvent::CopySessionId),
            KeyCode::Char('n') => Some(PickerEvent::NewSession),
            KeyCode::Char('r') => Some(PickerEvent::Refresh),

            KeyCode::Char('q') => Some(PickerEvent::Exit),
            KeyCode::Esc => Some(PickerEvent::Exit),

            _ => None,
        }
    }
}

/// Get current working directory
fn get_cwd() -> Result<PathBuf, String> {
    std::env::current_dir().map_err(|e| format!("Failed to get current directory: {e}"))
}

/// Get sessions directory
fn get_sessions_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|e| format!("Failed to get HOME: {e}"))?;
    Ok(PathBuf::from(home).join(".codex/sessions"))
}

/// Extract enhanced session metadata from .jsonl file
fn extract_session_meta(
    path: &PathBuf,
) -> Result<(String, String, usize, String, usize, String), String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let reader = std::io::BufReader::new(file);
    let mut lines = reader.lines();

    let mut session_id = String::new();
    let mut cwd = String::new();
    let mut model = String::from("unknown");
    let mut last_role = String::from("-");
    let mut total_tokens = 0;

    // First pass: extract session metadata from first line
    if let Some(Ok(first_line)) = lines.next()
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&first_line)
        && let Some(payload) = json.get("payload")
    {
        session_id = payload
            .get("id")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| path.file_name().unwrap().to_string_lossy().to_string());

        cwd = payload
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .unwrap_or_default();

        model = payload
            .get("model")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| "unknown".to_string());
    }

    // Second pass: gather message metadata
    let parsed = collect_session_messages(path);
    if let Some(tokens) = parsed.total_tokens {
        total_tokens = tokens;
    }
    let dialog_messages: Vec<&ParsedMessage> = parsed
        .messages
        .iter()
        .filter(|m| matches!(m.role.as_str(), "User" | "Assistant"))
        .collect();
    let message_count = dialog_messages.len();
    if let Some(last) = dialog_messages.last() {
        last_role = last.role.clone();
    }

    if session_id.is_empty() {
        session_id = path.file_name().unwrap().to_string_lossy().to_string();
    }

    Ok((
        session_id,
        cwd,
        message_count,
        last_role,
        total_tokens,
        model,
    ))
}

#[derive(Clone, Debug)]
struct ParsedMessage {
    role: String,
    content: String,
    timestamp: Option<String>,
}

#[derive(Default)]
pub(crate) struct ParsedSessionData {
    messages: Vec<ParsedMessage>,
    total_tokens: Option<usize>,
}

pub(crate) fn collect_session_messages(path: &PathBuf) -> ParsedSessionData {
    let mut data = ParsedSessionData::default();
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return data,
    };

    let reader = std::io::BufReader::new(file);
    let mut first_line = true;
    let mut new_format = false;

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(line) => line,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            if first_line {
                first_line = false;
                if json.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
                    new_format = true;
                    if let Some(tokens) = extract_total_tokens(&json) {
                        data.total_tokens = Some(tokens);
                    }
                    continue;
                }
            }

            if let Some(tokens) = extract_total_tokens(&json) {
                data.total_tokens = Some(tokens);
            }

            let message = if new_format {
                parse_new_format_message(&json)
            } else {
                parse_legacy_format_message(&json)
            };

            if let Some(message) = message
                && !message.content.trim().is_empty()
            {
                data.messages.push(message);
            }
        }
    }

    data
}

/// Return the first User message's first `max_words` words as a snippet label.
/// Falls back to None if no user message is present or content is empty.
#[allow(dead_code)]
pub fn first_user_snippet(path: &PathBuf, max_words: usize) -> Option<String> {
    let data = collect_session_messages(path);
    let first_user = data
        .messages
        .iter()
        .find(|m| m.role == "User" && !m.content.trim().is_empty())?;
    let mut words = first_user
        .content
        .split_whitespace()
        .filter(|w| !w.is_empty());
    let mut taken: Vec<&str> = Vec::new();
    for _ in 0..max_words {
        if let Some(w) = words.next() {
            taken.push(w);
        } else {
            break;
        }
    }
    if taken.is_empty() {
        None
    } else {
        Some(taken.join(" "))
    }
}

/// Return the last User message's first `max_words` words as a snippet label.
/// Falls back to None if no user message is present or content is empty.
pub fn last_user_snippet(path: &PathBuf, max_words: usize) -> Option<String> {
    let data = collect_session_messages(path);
    let last_user = data
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "User" && !m.content.trim().is_empty())?;
    let mut words = last_user
        .content
        .split_whitespace()
        .filter(|w| !w.is_empty());
    let mut taken: Vec<&str> = Vec::new();
    for _ in 0..max_words {
        if let Some(w) = words.next() {
            taken.push(w);
        } else {
            break;
        }
    }
    if taken.is_empty() {
        None
    } else {
        Some(taken.join(" "))
    }
}

fn parse_new_format_message(json: &serde_json::Value) -> Option<ParsedMessage> {
    if json.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
        return None;
    }

    let payload = json.get("payload")?;
    let event_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let (role, content_value) = match event_type {
        "user_message" => ("User", payload.get("message")),
        "agent_message" => ("Assistant", payload.get("message")),
        _ => return None,
    };

    let content = content_value
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .or_else(|| extract_text_from_content(payload.get("content")))?;

    let timestamp = json
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("timestamp").and_then(|v| v.as_str()))
        .map(std::string::ToString::to_string);

    Some(ParsedMessage {
        role: role.to_string(),
        content,
        timestamp,
    })
}

fn parse_legacy_format_message(json: &serde_json::Value) -> Option<ParsedMessage> {
    let payload = json.get("payload");
    let raw_role = payload
        .and_then(|p| p.get("role").and_then(|v| v.as_str()))
        .or_else(|| json.get("role").and_then(|v| v.as_str()))
        .unwrap_or("");
    let role = normalize_role(raw_role);

    if role != "User" && role != "Assistant" && role != "System" {
        return None;
    }

    let content_node = payload
        .and_then(|p| p.get("content"))
        .or_else(|| json.get("content"));
    let content = extract_text_from_content(content_node)
        .or_else(|| {
            payload
                .and_then(|p| p.get("text").and_then(|v| v.as_str()))
                .map(std::string::ToString::to_string)
        })
        .or_else(|| {
            json.get("text")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string)
        })?;

    if content.trim().is_empty() {
        return None;
    }

    let timestamp = json
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| payload.and_then(|p| p.get("timestamp").and_then(|v| v.as_str())))
        .map(std::string::ToString::to_string);

    Some(ParsedMessage {
        role,
        content,
        timestamp,
    })
}

fn normalize_role(raw: &str) -> String {
    match raw.to_lowercase().as_str() {
        "user" => "User".to_string(),
        "assistant" | "agent" => "Assistant".to_string(),
        "system" => "System".to_string(),
        other => other.to_string(),
    }
}

fn extract_text_from_content(node: Option<&serde_json::Value>) -> Option<String> {
    let mut segments: Vec<String> = Vec::new();
    if let Some(value) = node {
        collect_text_segments(value, &mut segments);
    }
    if segments.is_empty() {
        None
    } else {
        Some(segments.join("\n"))
    }
}

fn collect_text_segments(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.to_string()),
        serde_json::Value::Array(items) => {
            for item in items {
                if let serde_json::Value::Object(map) = item {
                    if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                        out.push(text.to_string());
                    } else if let Some(content) = map.get("content") {
                        collect_text_segments(content, out);
                    }
                } else {
                    collect_text_segments(item, out);
                }
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                out.push(text.to_string());
            }
            if let Some(message) = map.get("message").and_then(|v| v.as_str()) {
                out.push(message.to_string());
            }
            if let Some(nested) = map.get("content") {
                collect_text_segments(nested, out);
            }
        }
        _ => {}
    }
}

fn extract_total_tokens(json: &serde_json::Value) -> Option<usize> {
    let payload = json.get("payload")?;

    if let Some(usage) = payload.get("usage")
        && let Some(total) = usage
            .get("total_tokens")
            .and_then(serde_json::Value::as_u64)
    {
        return Some(total as usize);
    }

    if let Some(info) = payload.get("info")
        && let Some(total) = info
            .get("total_token_usage")
            .and_then(|usage| usage.get("total_tokens"))
            .and_then(serde_json::Value::as_u64)
    {
        return Some(total as usize);
    }

    None
}

fn format_timestamp_for_display(timestamp: Option<&String>) -> String {
    if let Some(ts) = timestamp {
        if let Some((_, time)) = ts.split_once('T') {
            let trimmed = time.trim_end_matches('Z');
            let display = trimmed.split('.').next().unwrap_or(trimmed);
            if !display.is_empty() {
                return display.to_string();
            }
        } else if !ts.is_empty() {
            return ts.clone();
        }
    }

    "--:--".to_string()
}

/// Extract recent messages from a session file for preview
fn extract_recent_messages(path: &PathBuf, limit: usize) -> Vec<(String, String)> {
    let ParsedSessionData { messages, .. } = collect_session_messages(path);
    let dialog: Vec<&ParsedMessage> = messages
        .iter()
        .filter(|m| matches!(m.role.as_str(), "User" | "Assistant"))
        .collect();
    if dialog.is_empty() {
        return Vec::new();
    }

    let start = dialog.len().saturating_sub(limit);
    dialog[start..]
        .iter()
        .copied()
        .map(|m| (m.role.clone(), m.content.clone()))
        .collect()
}

/// Format relative time in human-readable format
fn format_relative_time(mtime: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let diff = now.saturating_sub(mtime);

    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 604800 {
        format!("{}d ago", diff / 86400)
    } else if diff < 2592000 {
        format!("{}w ago", diff / 604800)
    } else if diff < 31536000 {
        format!("{}mo ago", diff / 2592000)
    } else {
        format!("{}y ago", diff / 31536000)
    }
}

/// Get sessions in current working directory with enhanced metadata
pub fn get_cwd_sessions() -> Result<Vec<SessionInfo>, String> {
    let cwd_raw = get_cwd()?;
    let cwd = cwd_raw.canonicalize().unwrap_or(cwd_raw);
    let sessions_dir = get_sessions_dir()?;
    let mut sessions = Vec::new();

    fn find_sessions(
        dir: &Path,
        cwd: &Path,
        sessions: &mut Vec<SessionInfo>,
        max_depth: u32,
    ) -> Result<(), String> {
        if max_depth == 0 {
            return Ok(());
        }

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
                    if let Ok((id, session_cwd, msg_count, last_role, tokens, model)) =
                        extract_session_meta(&path)
                        && should_include_session(&session_cwd, cwd)
                    {
                        let mtime = entry
                            .metadata()
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                            .unwrap_or(0);

                        let age = format_relative_time(mtime);

                        sessions.push(SessionInfo {
                            id,
                            path: path.clone(),
                            cwd: session_cwd,
                            age,
                            mtime,
                            message_count: msg_count,
                            last_role,
                            total_tokens: tokens,
                            model,
                            tumix: None,
                        });
                    }
                } else if path.is_dir() {
                    let _ = find_sessions(path.as_path(), cwd, sessions, max_depth - 1);
                }
            }
        }

        Ok(())
    }

    find_sessions(&sessions_dir, &cwd, &mut sessions, 4)?;

    // Sort by modification time (newest first)
    sessions.sort_by(|a, b| b.mtime.cmp(&a.mtime));

    sessions.retain(|session| session.message_count > 0);

    // Limit to recent 100 sessions for performance
    sessions.truncate(100);

    if sessions.is_empty() {
        Err("No sessions found in current working directory".to_string())
    } else {
        Ok(sessions)
    }
}

fn should_include_session(session_cwd: &str, cwd: &Path) -> bool {
    if session_cwd.is_empty() {
        return false;
    }

    let raw_path = PathBuf::from(session_cwd);
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else {
        cwd.join(raw_path)
    };

    match candidate.canonicalize() {
        Ok(real_path) => real_path == cwd || real_path.starts_with(cwd),
        Err(_) => false,
    }
}

fn has_running_tumix(sessions: &[SessionInfo]) -> bool {
    sessions.iter().any(|session| {
        matches!(
            session.tumix.as_ref().map(|t| t.state),
            Some(TumixState::Running)
        )
    })
}

#[derive(Debug, Deserialize)]
struct TumixSessionRecord {
    agent_id: String,
    agent_name: String,
    status: TumixStatusRaw,
    branch: String,
    session_id: Option<String>,
    #[allow(dead_code)]
    commit: Option<String>,
    jsonl_path: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TumixStatusRaw {
    Running,
    Completed,
    Failed,
}

pub fn load_tumix_status_index() -> TumixStatusIndex {
    let mut index = TumixStatusIndex::default();
    let tumix_dir = PathBuf::from(".tumix");
    let dir_iter = match fs::read_dir(&tumix_dir) {
        Ok(iter) => iter,
        Err(_) => return index,
    };

    let now = SystemTime::now();

    for entry in dir_iter.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if !matches!(
            path.file_name().and_then(OsStr::to_str),
            Some(name) if name.starts_with("round1_sessions_") && name.ends_with(".json")
        ) {
            continue;
        }

        let run_id = path
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| name.strip_prefix("round1_sessions_"))
            .and_then(|rest| rest.strip_suffix(".json"))
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| "unknown".to_string());

        let metadata = entry.metadata().ok();
        let modified = metadata.and_then(|meta| meta.modified().ok());

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };

        let records: Vec<TumixSessionRecord> = match serde_json::from_str(&content) {
            Ok(records) => records,
            Err(_) => continue,
        };

        for record in records {
            let session_id = match record.session_id.as_ref() {
                Some(id) => id,
                None => continue,
            };

            let mut state = match record.status {
                TumixStatusRaw::Running => TumixState::Running,
                TumixStatusRaw::Completed => TumixState::Completed,
                TumixStatusRaw::Failed => TumixState::Failed,
            };

            if state == TumixState::Running
                && let Some(modified) = modified
                && now
                    .duration_since(modified)
                    .unwrap_or(Duration::from_secs(0))
                    > Duration::from_secs(300)
            {
                state = TumixState::Stalled;
            }

            let agent_name = {
                let name = record.agent_name.trim();
                if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                }
            };

            let branch = {
                let branch = record.branch.trim();
                if branch.is_empty() {
                    None
                } else {
                    Some(branch.to_string())
                }
            };

            let error = record
                .error
                .as_ref()
                .map(|e| e.trim())
                .filter(|e| !e.is_empty())
                .map(std::string::ToString::to_string);

            let path_opt = record.jsonl_path.as_ref().map(|p| {
                let raw = PathBuf::from(p);
                match raw.canonicalize() {
                    Ok(real) => real,
                    Err(_) => raw,
                }
            });

            let indicator = TumixIndicator {
                run_id: run_id.clone(),
                agent_id: record.agent_id.clone(),
                agent_name,
                branch,
                state,
                error,
            };

            index.insert(session_id, path_opt, indicator, modified);
        }
    }

    index
}

#[allow(dead_code)]
/// Format left panel: session list with 3 lines per session
fn format_left_panel_sessions(
    sessions: &[SessionInfo],
    selected_idx: Option<usize>,
    _width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if sessions.is_empty() {
        lines.push("No sessions".yellow().into());
        return lines;
    }

    // Header with pagination info
    let total_sessions = sessions.len();
    let items_per_page = 30;
    let total_pages = total_sessions.div_ceil(items_per_page);
    let current_page = 1; // Default to page 1 for this renderer

    let header = format!(
        "SESSIONS  │  Page {}/{} │ Showing {}/{}",
        current_page,
        total_pages,
        sessions.len(),
        total_sessions
    );
    lines.push(ansi_escape_line(&header).bold());
    lines.push(Line::from(""));

    // Display sessions (3 lines per session + 1 blank line spacing)
    for (idx, session) in sessions.iter().enumerate() {
        let is_selected = selected_idx == Some(idx);
        let marker = if is_selected { "▶" } else { " " };

        // Line 1: marker, indicator, badge, ID, age
        let mut line1_spans = Vec::new();
        line1_spans.push(Span::from(" "));
        line1_spans.push(Span::from(marker.to_string()));
        line1_spans.push(Span::from(" "));
        line1_spans.push(tumix_indicator_span(session, 0));
        line1_spans.push(Span::from(" "));
        if session.tumix.is_some() {
            line1_spans.push(tumix_badge_span());
            line1_spans.push(Span::from(" "));
        }
        line1_spans.push(session_id_span(session.id.as_str()));
        line1_spans.push(Span::from("  "));
        line1_spans.push(session_age_span(session.age.as_str()));
        let mut line1 = Line::from(line1_spans);
        if is_selected {
            line1 = line1.reversed();
        }
        lines.push(line1);

        // Line 2: CWD or path
        let line2_text = format!("   {}", stylize_cwd(session.cwd.as_str()));
        let mut line2 = ansi_escape_line(&line2_text);
        if is_selected {
            line2 = line2.reversed();
        }
        lines.push(line2);

        // Line 3: Messages + Model + Last role
        let line3_text = format!(
            "   {messages_label}{messages_value}{sep1}{model_label}{model_value}{sep2}{last_label}{last_value}",
            messages_label = stylize_label("Messages: "),
            messages_value = stylize_messages_count(session.message_count),
            sep1 = stylize_separator(),
            model_label = stylize_label("Model: "),
            model_value = stylize_model_name(session.model.as_str()),
            sep2 = stylize_separator(),
            last_label = stylize_label("Last: "),
            last_value = stylize_last_role_text(session.last_role.as_str()),
        );
        let mut line3 = ansi_escape_line(&line3_text);
        if is_selected {
            line3 = line3.reversed();
        }
        lines.push(line3);

        // Spacing
        lines.push(Line::from(""));
    }

    lines
}

fn tumix_line_two_text(session: &SessionInfo) -> Option<String> {
    let indicator = session.tumix.as_ref()?;
    let mut parts: Vec<String> = Vec::new();
    let agent_label = format!("Agent {}", indicator.agent_id);
    parts.push(agent_label.as_str().dim().to_string());
    if let Some(agent) = &indicator.agent_name {
        parts.push(agent.as_str().bold().to_string());
    }
    if let Some(branch) = &indicator.branch {
        parts.push(branch.as_str().cyan().to_string());
    }
    parts.push(stylize_cwd(session.cwd.as_str()));
    Some(format!("   {}", parts.join(&stylize_separator())))
}

fn tumix_line_three_suffix(session: &SessionInfo) -> String {
    let indicator = match session.tumix.as_ref() {
        Some(indicator) => indicator,
        None => return String::new(),
    };

    let mut parts: Vec<String> = Vec::new();
    let run_label = format!("run {}", short_run_id(&indicator.run_id));
    parts.push(run_label.as_str().dim().to_string());

    if matches!(indicator.state, TumixState::Failed | TumixState::Stalled)
        && let Some(error) = indicator.error.as_ref()
    {
        let truncated = truncate_error(error);
        parts.push(truncated.as_str().red().to_string());
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(
            "{}{}",
            stylize_separator(),
            parts.join(&stylize_separator())
        )
    }
}

fn short_run_id(run_id: &str) -> String {
    if run_id.len() <= 8 {
        run_id.to_string()
    } else {
        run_id[run_id.len().saturating_sub(8)..].to_string()
    }
}

fn truncate_error(error: &str) -> String {
    let first_line = error.lines().next().unwrap_or("").trim();
    if first_line.len() <= 64 {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(61).collect();
        format!("{truncated}…")
    }
}

fn tumix_state_text(state: TumixState) -> String {
    match state {
        TumixState::Completed => "completed".green().bold().to_string(),
        TumixState::Failed => "failed".red().bold().to_string(),
        TumixState::Running => "running".yellow().bold().to_string(),
        TumixState::Stalled => "stalled".magenta().bold().to_string(),
    }
}
/// Format left panel with pagination state  - displays paginated session list with pagination info
#[allow(dead_code)]
fn format_left_panel_sessions_paginated(
    sessions: &[SessionInfo],
    state: &PickerState,
    _width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if sessions.is_empty() {
        lines.push("No sessions".yellow().into());
        return lines;
    }

    // Pagination information
    let total_pages = state.pagination.total_pages();
    let current_page = state.pagination.current_page + 1; // Display as 1-indexed
    let showing = sessions.len();

    let header = format!(
        "SESSIONS  │  Page {}/{} │ Showing {}/{}",
        current_page,
        total_pages,
        showing,
        state.sessions.len()
    );
    lines.push(ansi_escape_line(&header).bold());
    lines.push(Line::from(""));

    // Display sessions for current page (3 lines per session + 1 blank line spacing)
    for (page_idx, session) in sessions.iter().enumerate() {
        let abs_idx = state.pagination.page_start() + page_idx;
        let is_selected = state.selected_idx == abs_idx;
        let marker = if is_selected { "▶" } else { " " };

        let mut line1_spans = Vec::new();
        line1_spans.push(Span::from(" "));
        line1_spans.push(Span::from(marker.to_string()));
        line1_spans.push(Span::from(" "));
        line1_spans.push(tumix_indicator_span(session, state.animation_frame()));
        line1_spans.push(Span::from(" "));
        if session.tumix.is_some() {
            line1_spans.push(tumix_badge_span());
            line1_spans.push(Span::from(" "));
        }
        line1_spans.push(session_id_span(session.id.as_str()));
        line1_spans.push(Span::from("  "));
        line1_spans.push(session_age_span(session.age.as_str()));

        let mut line1 = Line::from(line1_spans);
        if is_selected {
            line1 = line1.reversed();
        }
        lines.push(line1);

        let line2_text = tumix_line_two_text(session)
            .unwrap_or_else(|| format!("   {}", stylize_cwd(session.cwd.as_str())));
        let mut line2 = ansi_escape_line(&line2_text);
        if is_selected {
            line2 = line2.reversed();
        }
        lines.push(line2);

        let base_line3 = format!(
            "   {messages_label}{messages_value}{sep1}{model_label}{model_value}{sep2}{last_label}{last_value}",
            messages_label = stylize_label("Messages: "),
            messages_value = stylize_messages_count(session.message_count),
            sep1 = stylize_separator(),
            model_label = stylize_label("Model: "),
            model_value = stylize_model_name(session.model.as_str()),
            sep2 = stylize_separator(),
            last_label = stylize_label("Last: "),
            last_value = stylize_last_role_text(session.last_role.as_str()),
        );
        let line3_text = format!("{base_line3}{}", tumix_line_three_suffix(session));
        let mut line3 = ansi_escape_line(&line3_text);
        if is_selected {
            line3 = line3.reversed();
        }
        lines.push(line3);

        lines.push(Line::from(""));
    }

    lines
}

/// Format right panel: message preview with block-style format
fn format_right_panel_preview(session: &SessionInfo, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let effective_width = width.max(1);

    lines.push(ansi_escape_line(&format!(
        "▸ Session: {session_id} │ Path: {session_path}",
        session_id = stylize_session_id(session.id.as_str()),
        session_path = stylize_cwd(session.cwd.as_str()),
    )));
    lines.push(ansi_escape_line(&format!(
        "▸ {model_label}{model_value} │ {messages_label}{messages_value} │ {last_label}{last_value}",
        model_label = stylize_label("Model: "),
        model_value = stylize_model_name(session.model.as_str()),
        messages_label = stylize_label("Messages: "),
        messages_value = stylize_messages_count(session.message_count),
        last_label = stylize_label("Last: "),
        last_value = stylize_last_role_text(session.last_role.as_str()),
    )));
    lines.push(Line::from(""));

    let separator = "─".repeat(effective_width as usize);
    lines.push(Line::from(separator).dim());
    lines.push(Line::from(""));

    let mut messages = extract_recent_messages_with_timestamps(&session.path, 6);
    messages.retain(|(_, content, _)| !content.trim().is_empty());
    if messages.is_empty() {
        lines.push("Select a session to preview messages".dim().into());
        return lines;
    }

    let content_width = effective_width.saturating_sub(4).max(1) as usize;

    for (idx, (role, content, timestamp)) in messages.iter().enumerate() {
        let index = idx + 1;
        let role_color = dialog_role_color(role);
        let role_text = format!("{index}. {role_text}", role_text = role.as_str());
        let header_spans = vec![
            Span::styled("┃ ".to_string(), Style::default().fg(role_color)),
            Span::styled(role_text, Style::default().fg(role_color)),
            " ".into(),
            Span::styled(timestamp.clone(), Style::default().fg(THEME_GRAY)),
        ];
        lines.push(Line::from(header_spans));

        let sanitized = content.replace('\r', "");

        let wrapped_raw = crate::wrapping::word_wrap_lines(
            &vec![Line::from(sanitized)],
            crate::wrapping::RtOptions::new(content_width)
                .initial_indent("  ".into())
                .subsequent_indent("  ".into()),
        );
        let wrapped_colored: Vec<Line<'static>> = wrapped_raw
            .into_iter()
            .map(|line| line.fg(role_color))
            .collect();

        let prefixed = line_utils::prefix_lines(
            wrapped_colored,
            colored_bar_span(role_color),
            colored_bar_span(role_color),
        );
        if prefixed.is_empty() {
            lines.push(
                Line::from(vec![colored_bar_span(role_color), Span::from("  ")]).fg(role_color),
            );
        } else {
            lines.extend(prefixed);
        }

        lines.push(Line::from(""));
    }

    lines
}

/// Format the help/legend section with key bindings and information
#[allow(dead_code)]
fn format_help_section() -> Vec<Line<'static>> {
    vec![
        Line::from(""),
        Line::from("────────────────────────────────────────────────────────────────").dim(),
        Line::from(""),
        "Key Bindings:".bold().into(),
        ansi_escape_line("  ↑↓ / j/k  Navigate sessions       Enter  Resume selected session"),
        ansi_escape_line("  i         Session info           p      Preview messages"),
        ansi_escape_line("  d         Delete session         r      Refresh session list"),
        ansi_escape_line("  q / Esc   Close this panel       /      Search sessions"),
        Line::from(""),
        "Display Information:".bold().into(),
        ansi_escape_line("  • Messages: Total user + assistant messages in this session"),
        ansi_escape_line("  • Last: Last message type in session (User or Assistant)"),
        ansi_escape_line("  • Model: AI model used for this session"),
        ansi_escape_line("  • Tokens: Total tokens consumed in this session"),
        Line::from(""),
        Line::from("────────────────────────────────────────────────────────────────").dim(),
    ]
}

/// Format detailed session information for info modal
#[allow(dead_code)]
fn format_session_details(session: &SessionInfo) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(""));
    lines.push("SESSION DETAILS".bold().cyan().into());
    lines.push(Line::from(""));

    // Session ID
    lines.push(ansi_escape_line(&format!(
        "  ID:             {}",
        session.id.as_str().cyan()
    )));

    // Model
    lines.push(ansi_escape_line(&format!(
        "  Model:          {}",
        session.model.as_str().yellow()
    )));

    // Messages
    lines.push(ansi_escape_line(&format!(
        "  Messages:       {} ({} last)",
        session.message_count.to_string().yellow(),
        session.last_role.as_str().green()
    )));

    // Tokens
    lines.push(ansi_escape_line(&format!(
        "  Tokens Used:    {}",
        session.total_tokens.to_string().yellow()
    )));

    // Age
    lines.push(ansi_escape_line(&format!(
        "  Last Activity:  {}",
        session.age.as_str().dim()
    )));

    // Working Directory
    if !session.cwd.is_empty() {
        lines.push(ansi_escape_line(&format!(
            "  Directory:      {}",
            session.cwd.as_str().dim()
        )));
    }

    // File Path
    lines.push(ansi_escape_line(&format!(
        "  File Path:      {}",
        session.path.display().to_string().as_str().dim()
    )));

    if let Some(tumix) = &session.tumix {
        lines.push(Line::from(""));
        lines.push("TUMIX".bold().magenta().into());
        lines.push(ansi_escape_line(&format!(
            "  State:          {}",
            tumix_state_text(tumix.state)
        )));
        lines.push(ansi_escape_line(&format!(
            "  Run ID:         {}",
            tumix.run_id.as_str().dim()
        )));
        lines.push(ansi_escape_line(&format!(
            "  Agent ID:       {}",
            tumix.agent_id.as_str().cyan()
        )));
        if let Some(agent) = tumix.agent_name.as_deref() {
            lines.push(ansi_escape_line(&format!(
                "  Agent:          {}",
                agent.bold()
            )));
        }
        if let Some(branch) = tumix.branch.as_deref() {
            lines.push(ansi_escape_line(&format!(
                "  Branch:         {}",
                branch.cyan()
            )));
        }
        if let Some(error) = tumix.error.as_deref() {
            let truncated = truncate_error(error);
            lines.push(ansi_escape_line(&format!(
                "  Notes:          {}",
                truncated.as_str().red()
            )));
        }
    }

    lines.push(Line::from(""));
    lines
        .push(Line::from("────────────────────────────────────────────────────────────────").dim());
    lines.push(Line::from(""));
    lines.push("STATISTICS".bold().cyan().into());
    lines.push(Line::from(""));

    lines.push(ansi_escape_line(&format!(
        "  Average tokens per message: {}",
        if session.message_count > 0 {
            (session.total_tokens / session.message_count).to_string()
        } else {
            "N/A".to_string()
        }
        .as_str()
        .yellow()
    )));

    lines.push(Line::from(""));
    lines.push("Actions:".bold().into());
    lines.push(ansi_escape_line("  Enter      Resume this session"));
    lines.push(ansi_escape_line("  p          Preview recent messages"));
    lines.push(ansi_escape_line("  d          Delete this session"));
    lines.push(ansi_escape_line("  q / Esc    Back to session list"));
    lines.push(Line::from(""));

    lines
}

/// Extract messages with timestamps (role, content, timestamp)
fn extract_recent_messages_with_timestamps(
    path: &PathBuf,
    limit: usize,
) -> Vec<(String, String, String)> {
    let ParsedSessionData { messages, .. } = collect_session_messages(path);
    let dialog: Vec<&ParsedMessage> = messages
        .iter()
        .filter(|m| matches!(m.role.as_str(), "User" | "Assistant"))
        .collect();
    if dialog.is_empty() {
        return Vec::new();
    }

    let start = dialog.len().saturating_sub(limit);
    dialog[start..]
        .iter()
        .copied()
        .map(|m| {
            (
                m.role.clone(),
                m.content.clone(),
                format_timestamp_for_display(m.timestamp.as_ref()),
            )
        })
        .collect()
}

/// Format message blocks with vertical bar indicator (┃)
/// This creates the block-style preview format used in cxresume JS
#[allow(dead_code)]
fn format_message_blocks(session: &SessionInfo, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Info box header
    let info_text = format!(
        "Session: {} • Path: {} • Started: {}",
        session.id.as_str().cyan(),
        session.cwd.as_str().dim(),
        session.age.as_str().dim()
    );
    lines.push(ansi_escape_line(&info_text));
    lines.push(Line::from(""));

    let messages = extract_recent_messages_with_timestamps(&session.path, 8);
    if messages.is_empty() {
        lines.push("No messages found in this session.".yellow().into());
    } else {
        for (role, content, _timestamp) in messages.iter() {
            // Message header with bar indicator (┃)
            let role_color = if role == "User" {
                role.as_str().red()
            } else {
                role.as_str().green()
            };

            let header_text = format!("┃ {role_color}");
            lines.push(ansi_escape_line(&header_text));

            // Message body with wrapping and bar prefix
            let usable_width = width.saturating_sub(3) as usize; // Account for "┃ " prefix
            for content_line in content.lines() {
                // Wrap long lines
                if content_line.len() > usable_width {
                    let mut remaining = content_line;
                    while !remaining.is_empty() {
                        let chunk_size = usable_width.min(remaining.len());
                        let chunk = &remaining[..chunk_size];
                        lines.push(ansi_escape_line(&format!("  {chunk}")));
                        remaining = &remaining[chunk_size..];
                    }
                } else {
                    lines.push(ansi_escape_line(&format!("  {content_line}")));
                }
            }

            // Spacing between messages
            lines.push(Line::from(""));
        }
    }

    lines
}

/// Format session preview with recent messages
fn format_session_preview(session: &SessionInfo) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(""));
    lines.push("SESSION PREVIEW - Recent Messages".bold().cyan().into());
    lines.push(Line::from(""));
    lines.push(ansi_escape_line(&format!(
        "Session: {}",
        session.id.as_str().cyan()
    )));
    lines.push(ansi_escape_line(&format!(
        "Model: {}",
        session.model.as_str().yellow()
    )));
    lines.push(Line::from(""));
    lines
        .push(Line::from("────────────────────────────────────────────────────────────────").dim());
    lines.push(Line::from(""));

    let mut messages = extract_recent_messages(&session.path, 5);
    messages.retain(|(_, content)| !content.trim().is_empty());
    if messages.is_empty() {
        lines.push("Select a session to preview messages".dim().into());
    } else {
        for (idx, (role, content)) in messages.iter().enumerate() {
            // Role header
            let role_line = if role == "User" {
                format!("{}. {} (User)", idx + 1, "▶".cyan())
            } else {
                format!("{}. {} (Assistant)", idx + 1, "◀".green())
            };
            lines.push(role_line.into());

            let sanitized = content.replace('\r', "");
            let wrapped = crate::wrapping::word_wrap_lines(
                &vec![Line::from(sanitized)],
                crate::wrapping::RtOptions::new(FULL_PREVIEW_WRAP_WIDTH)
                    .initial_indent("   ".into())
                    .subsequent_indent("   ".into()),
            );

            for line in wrapped {
                lines.push(line.dim());
            }
            lines.push(Line::from(""));
        }
    }

    lines
        .push(Line::from("────────────────────────────────────────────────────────────────").dim());
    lines.push(Line::from(""));
    lines.push("  q / Esc  Back to session list".dim().into());
    lines.push(Line::from(""));

    lines
}

/// Build picker state for the current working directory.
pub fn load_picker_state() -> Result<PickerState, String> {
    let mut sessions = get_cwd_sessions()?;
    let tumix_index = load_tumix_status_index();
    for session in &mut sessions {
        if let Some(indicator) = tumix_index.lookup(&session.id, &session.path) {
            session.tumix = Some(indicator);
        }
    }
    Ok(PickerState::new(sessions))
}

/// Create a comprehensive Split View session picker overlay
pub fn create_session_picker_overlay() -> Result<Overlay, String> {
    let state = load_picker_state()?;
    let picker_overlay = crate::pager_overlay::SessionPickerOverlay::from_state(state);
    Ok(Overlay::SessionPicker(picker_overlay))
}

/// Render the picker view directly into the provided frame.
pub fn render_picker_view(frame: &mut crate::custom_terminal::Frame, state: &PickerState) {
    let header_lines = build_header_lines(state);
    let footer_lines = build_footer_lines(state);
    let header_height = header_lines.len() as u16;
    let footer_height = footer_lines.len() as u16;

    let mut constraints = Vec::new();
    let mut header_index = None;
    let mut footer_index = None;

    if header_height > 0 {
        header_index = Some(constraints.len());
        constraints.push(Constraint::Length(header_height));
    }

    let body_index = constraints.len();
    constraints.push(Constraint::Min(3));

    if footer_height > 0 {
        footer_index = Some(constraints.len());
        constraints.push(Constraint::Length(footer_height));
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    if let Some(idx) = header_index {
        Paragraph::new(header_lines).render(sections[idx], frame.buffer_mut());
    }

    render_body(frame, sections[body_index], state);

    if let Some(idx) = footer_index {
        Paragraph::new(footer_lines).render(sections[idx], frame.buffer_mut());
    }
}

fn render_body(frame: &mut crate::custom_terminal::Frame, area: Rect, state: &PickerState) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    if state.sessions.is_empty() {
        let lines = vec![
            "No sessions found in current working directory"
                .yellow()
                .into(),
            Line::from(""),
            "Press q to close".dim().into(),
        ];
        Paragraph::new(lines).render(area, frame.buffer_mut());
        return;
    }

    match state.view_mode {
        ViewMode::Split => render_split_body(frame, area, state),
        ViewMode::FullPreview => render_full_preview_body(frame, area, state),
        ViewMode::SessionOnly => render_session_list_body(frame, area, state),
    }
}

fn render_split_body(frame: &mut crate::custom_terminal::Frame, area: Rect, state: &PickerState) {
    if area.width <= 2 || area.height == 0 {
        let fallback = state
            .selected_session()
            .map(|session| format_right_panel_preview(session, area.width))
            .unwrap_or_else(|| vec!["Select a session to preview messages".dim().into()]);
        Paragraph::new(fallback).render(area, frame.buffer_mut());
        return;
    }

    let split = SplitLayout::new(area.width, area.height);
    let regions = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(split.left_width),
            Constraint::Length(split.gap),
            Constraint::Length(split.right_width),
        ])
        .split(area);

    let left_lines_full = format_left_panel_sessions_paginated(
        state.current_page_sessions(),
        state,
        regions[0].width,
    );
    let left_visible = slice_left_panel_lines(&left_lines_full, regions[0].height as usize);
    let left_border_style = if state.focus == FocusPane::LeftList {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    Paragraph::new(left_visible)
        .block(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(left_border_style),
        )
        .render(regions[0], frame.buffer_mut());

    let right_width = regions[2].width.max(1);
    let right_lines = state
        .selected_session()
        .map(|session| format_right_panel_preview(session, right_width))
        .unwrap_or_else(|| vec!["Select a session to preview messages".dim().into()]);
    let right_visible = slice_preview_lines(
        right_lines,
        regions[2].height as usize,
        state.scroll_offset_right,
    );
    let right_border_style = if state.focus == FocusPane::RightPreview {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    Paragraph::new(right_visible)
        .block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(right_border_style),
        )
        .render(regions[2], frame.buffer_mut());
}

fn render_full_preview_body(
    frame: &mut crate::custom_terminal::Frame,
    area: Rect,
    state: &PickerState,
) {
    let lines = state
        .selected_session()
        .map(format_session_preview)
        .unwrap_or_else(|| vec!["No session selected".yellow().into()]);
    let offset = if state.focus == FocusPane::RightPreview {
        state.scroll_offset_right
    } else {
        0
    };
    let visible = slice_preview_lines(lines, area.height as usize, offset);
    Paragraph::new(visible).render(area, frame.buffer_mut());
}

fn render_session_list_body(
    frame: &mut crate::custom_terminal::Frame,
    area: Rect,
    state: &PickerState,
) {
    let lines_full =
        format_left_panel_sessions_paginated(state.current_page_sessions(), state, area.width);
    let visible = slice_left_panel_lines(&lines_full, area.height as usize);
    Paragraph::new(visible).render(area, frame.buffer_mut());
}

fn slice_left_panel_lines(lines: &[Line<'static>], height: usize) -> Vec<Line<'static>> {
    if height == 0 || lines.is_empty() {
        return Vec::new();
    }

    if lines.len() <= height {
        return lines.to_vec();
    }

    let header_count = lines.len().min(2);
    let mut result: Vec<Line<'static>> = Vec::new();

    let header_visible = header_count.min(height);
    if header_visible > 0 {
        result.extend_from_slice(&lines[..header_visible]);
    }

    let remaining_height = height.saturating_sub(header_visible);
    if remaining_height == 0 {
        return result;
    }

    let body = &lines[header_count..];
    if body.is_empty() {
        return result;
    }

    let focus = focus_line_index(body);
    let body_slice = slice_lines_with_focus(body, remaining_height, focus);
    result.extend(body_slice);
    result
}

fn focus_line_index(lines: &[Line<'static>]) -> usize {
    lines.iter().position(line_has_reverse).unwrap_or(0)
}

fn line_has_reverse(line: &Line<'static>) -> bool {
    if line.style.add_modifier.contains(Modifier::REVERSED) {
        return true;
    }
    line.spans
        .iter()
        .any(|span| span.style.add_modifier.contains(Modifier::REVERSED))
}

fn slice_lines_with_focus(
    lines: &[Line<'static>],
    height: usize,
    focus_line: usize,
) -> Vec<Line<'static>> {
    if height == 0 || lines.is_empty() {
        return Vec::new();
    }

    if lines.len() <= height {
        return lines.to_vec();
    }

    let focus = focus_line.min(lines.len().saturating_sub(1));
    let mut start = focus.saturating_sub(height / 2);
    if start + height > lines.len() {
        start = lines.len() - height;
    }
    let end = start + height;
    lines[start..end].to_vec()
}

fn slice_preview_lines(
    lines: Vec<Line<'static>>,
    height: usize,
    offset: usize,
) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }
    if lines.len() <= height {
        return lines;
    }

    let max_offset = lines.len().saturating_sub(height);
    let start = offset.min(max_offset);
    lines.into_iter().skip(start).take(height).collect()
}

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    copy_to_clipboard_impl(text)
}

#[cfg(not(target_os = "android"))]
fn copy_to_clipboard_impl(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
fn copy_to_clipboard_impl(_: &str) -> Result<(), String> {
    Err("Clipboard is not supported on this platform".to_string())
}

fn build_header_lines(state: &PickerState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    let focus_label = match state.focus {
        FocusPane::LeftList => "Left",
        FocusPane::RightPreview => "Preview",
    };
    let title = format!(
        "    C X R E S U M E   S E S S I O N   P I C K E R    ({session_count} sessions) │ Mode: {mode:?} │ Focus: {focus}",
        session_count = state.sessions.len(),
        mode = state.view_mode,
        focus = focus_label
    );
    lines.push(ansi_escape_line(&title).bold().cyan());
    lines.push(Line::from(""));
    lines
}

fn build_footer_lines(state: &PickerState) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));

    if state.modal_active {
        lines.push(
            Line::from("╔════════════════════════════════════════════════════════════╗").dim(),
        );
        for line in state.modal_message.lines() {
            lines.push(ansi_escape_line(&format!("║ {line} ")).dim());
        }
        lines.push(
            Line::from("╚════════════════════════════════════════════════════════════╝").dim(),
        );
        lines.push(Line::from(""));
    }

    lines
        .push(Line::from("────────────────────────────────────────────────────────────────").dim());
    lines.push(Line::from("Keyboard Shortcuts:").bold());
    lines.push(ansi_escape_line(
        "  ↑↓      Navigate sessions       Page↑/↓  Page jump       ←→      Switch focus",
    ));
    lines.push(ansi_escape_line(
        "  Enter   Resume session         d      Delete              f        Full preview",
    ));
    lines.push(ansi_escape_line(
        "  n       New session            c      Copy ID             q/Esc    Close",
    ));
    lines.push(Line::from(""));
    lines
}
