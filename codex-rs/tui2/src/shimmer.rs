use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Span;

use crate::color::blend;
use crate::terminal_palette::default_bg;
use crate::terminal_palette::default_fg;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Global flag to disable shimmer animation for low GPU mode.
/// When true, shimmer_spans returns simple bold text instead of RGB animation.
static SHIMMER_DISABLED: AtomicBool = AtomicBool::new(false);

/// Disable shimmer animation globally to reduce GPU usage.
pub fn disable_shimmer() {
    SHIMMER_DISABLED.store(true, Ordering::Relaxed);
}

/// Check if shimmer is currently disabled.
pub fn is_shimmer_disabled() -> bool {
    SHIMMER_DISABLED.load(Ordering::Relaxed)
}

fn elapsed_since_start() -> Duration {
    let start = PROCESS_START.get_or_init(Instant::now);
    start.elapsed()
}

/// Returns simple non-animated spans when shimmer is disabled.
/// This significantly reduces GPU load by avoiding per-character RGB colors.
pub(crate) fn simple_spans(text: &str) -> Vec<Span<'static>> {
    vec![Span::styled(
        text.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )]
}

pub(crate) fn shimmer_spans(text: &str) -> Vec<Span<'static>> {
    // Fast path: return simple spans if shimmer is disabled
    if is_shimmer_disabled() {
        return simple_spans(text);
    }

    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    // Use time-based sweep synchronized to process start.
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    // Slower sweep (3s instead of 2s) reduces the visual change rate and GPU load
    let sweep_seconds = 3.0f32;
    let pos_f =
        (elapsed_since_start().as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32);
    let pos = pos_f as usize;
    let has_true_color = supports_color::on_cached(supports_color::Stream::Stdout)
        .map(|level| level.has_16m)
        .unwrap_or(false);
    let band_half_width = 5.0;

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(chars.len());
    let base_color = default_fg().unwrap_or((128, 128, 128));
    let highlight_color = default_bg().unwrap_or((255, 255, 255));
    for (i, ch) in chars.iter().enumerate() {
        let i_pos = i as isize + padding as isize;
        let pos = pos as isize;
        let dist = (i_pos - pos).abs() as f32;

        let t = if dist <= band_half_width {
            let x = std::f32::consts::PI * (dist / band_half_width);
            0.5 * (1.0 + x.cos())
        } else {
            0.0
        };
        let style = if has_true_color {
            let highlight = t.clamp(0.0, 1.0);
            let (r, g, b) = blend(highlight_color, base_color, highlight * 0.9);
            // Allow custom RGB colors, as the implementation is thoughtfully
            // adjusting the level of the default foreground color.
            #[allow(clippy::disallowed_methods)]
            {
                Style::default()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD)
            }
        } else {
            color_for_level(t)
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

fn color_for_level(intensity: f32) -> Style {
    // Tune fallback styling so the shimmer band reads even without RGB support.
    if intensity < 0.2 {
        Style::default().add_modifier(Modifier::DIM)
    } else if intensity < 0.6 {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}
