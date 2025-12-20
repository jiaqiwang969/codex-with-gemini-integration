use std::collections::VecDeque;

use ratatui::text::Line;

use crate::markdown_stream::MarkdownStreamCollector;
pub(crate) mod controller;

/// Maximum number of queued lines to prevent unbounded memory growth.
/// This should be large enough to handle burst streaming but bounded.
const MAX_QUEUED_LINES: usize = 10000;

pub(crate) struct StreamState {
    pub(crate) collector: MarkdownStreamCollector,
    queued_lines: VecDeque<Line<'static>>,
    pub(crate) has_seen_delta: bool,
}

impl StreamState {
    pub(crate) fn new(width: Option<usize>) -> Self {
        Self {
            collector: MarkdownStreamCollector::new(width),
            queued_lines: VecDeque::new(),
            has_seen_delta: false,
        }
    }
    pub(crate) fn clear(&mut self) {
        self.collector.clear();
        self.queued_lines.clear();
        self.has_seen_delta = false;
    }
    pub(crate) fn step(&mut self) -> Vec<Line<'static>> {
        self.queued_lines.pop_front().into_iter().collect()
    }
    pub(crate) fn drain_all(&mut self) -> Vec<Line<'static>> {
        self.queued_lines.drain(..).collect()
    }
    pub(crate) fn is_idle(&self) -> bool {
        self.queued_lines.is_empty()
    }
    pub(crate) fn enqueue(&mut self, lines: Vec<Line<'static>>) {
        // Limit queue size to prevent unbounded memory growth
        let available = MAX_QUEUED_LINES.saturating_sub(self.queued_lines.len());
        if available > 0 {
            self.queued_lines.extend(lines.into_iter().take(available));
        }
    }
}
