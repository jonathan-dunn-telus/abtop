use crate::app::App;
use crate::locale::t;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::VecDeque;
use std::time::Instant;

use super::{braille_graph_multirow, btop_block_active, fmt_tokens, grad_at, make_gradient};

pub(crate) fn draw_token_rate_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    let block = btop_block_active("token rate", "⁹", theme.cpu_box, theme, active);
    let inner = block.inner(area);
    f.render_widget(block, area);
    draw_token_rate_charts(f, app, inner, theme, Instant::now());
}

fn draw_token_rate_charts(f: &mut Frame, app: &App, area: Rect, theme: &Theme, now: Instant) {
    if area.is_empty() {
        return;
    }
    let cpu_grad = make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);
    let cached_grad = make_gradient(
        theme.cached_grad.start,
        theme.cached_grad.mid,
        theme.cached_grad.end,
    );
    let rates = app.global_token_rates(now);
    let height = area.height as usize;
    let width = area.width as usize;
    let mut main_label = rate_label(
        t("token_rate.global"),
        rates.total(),
        series_color(&app.token_rates, &cpu_grad),
        theme,
    );
    let mut lines = Vec::new();
    if !app.show_cache_read_rate {
        lines.push(main_label);
        lines.extend(graph_lines(
            &app.token_rates,
            width,
            height - 1,
            &cpu_grad,
            theme,
        ));
    } else {
        let reuse = rates
            .cache_reuse_percent()
            .map(|pct| format!("{pct:.0}%"))
            .unwrap_or_else(|| "—".to_string());
        let reuse_label = t("token_rate.reuse");
        let mut cache_label = rate_label(
            t("token_rate.cache_reads"),
            rates.cache_read,
            series_color(&app.cache_read_rates, &cached_grad),
            theme,
        );
        cache_label.spans.push(Span::styled(
            format!(" · {reuse_label} {reuse}"),
            Style::default().fg(theme.graph_text),
        ));
        match height {
            1 => lines.push(Line::styled(
                format!(
                    "{} {}/min | CacheR {}/min | {reuse_label} {reuse}",
                    t("token_rate.global"),
                    fmt_tokens(rates.total()),
                    fmt_tokens(rates.cache_read),
                ),
                Style::default().fg(theme.graph_text),
            )),
            2 | 3 => {
                lines.push(main_label);
                lines.extend(graph_lines(
                    &app.token_rates,
                    width,
                    height - 2,
                    &cpu_grad,
                    theme,
                ));
                lines.push(cache_label);
            }
            _ => {
                main_label.spans.push(Span::styled(
                    format!(" · {}", t("token_rate.independent_scales")),
                    Style::default().fg(theme.graph_text),
                ));
                let graph_rows = height - 2;
                lines.push(main_label);
                lines.extend(graph_lines(
                    &app.token_rates,
                    width,
                    graph_rows.div_ceil(2),
                    &cpu_grad,
                    theme,
                ));
                lines.push(cache_label);
                lines.extend(graph_lines(
                    &app.cache_read_rates,
                    width,
                    graph_rows / 2,
                    &cached_grad,
                    theme,
                ));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn rate_label(label: String, rate: u64, color: Color, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(label, Style::default().fg(theme.graph_text)),
        Span::styled(
            format!("  {}/min", fmt_tokens(rate)),
            Style::default().fg(color),
        ),
    ])
}

fn series_max(data: &VecDeque<f64>) -> f64 {
    data.iter().copied().fold(1.0_f64, f64::max)
}

fn series_color(data: &VecDeque<f64>, gradient: &[Color; 101]) -> Color {
    grad_at(
        gradient,
        data.back().copied().unwrap_or(0.0) / series_max(data) * 100.0,
    )
}

fn graph_lines(
    data: &VecDeque<f64>,
    width: usize,
    height: usize,
    gradient: &[Color; 101],
    theme: &Theme,
) -> Vec<Line<'static>> {
    let max = series_max(data);
    let normalized: Vec<f64> = data.iter().map(|value| value / max).collect();
    braille_graph_multirow(
        &normalized,
        width.saturating_sub(2),
        height,
        gradient,
        theme.graph_text,
    )
    .into_iter()
    .map(|row| {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(row);
        Line::from(spans)
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::TokenRates;
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
    use std::time::Duration;

    fn render(app: &App, height: u16, now: Instant) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(60, height)).unwrap();
        terminal
            .draw(|f| draw_token_rate_charts(f, app, f.area(), &app.theme, now))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn charts_use_independent_scales_and_aligned_sample_positions() {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        app.show_cache_read_rate = true;
        let now = Instant::now();
        app.record_global_token_delta(
            now,
            TokenRates {
                input: 100,
                cache_read: 10_000,
                ..Default::default()
            },
        );
        app.record_global_token_delta(now, TokenRates::default());
        let buffer = render(&app, 6, now);
        assert!(row(&buffer, 0).contains("100/min"));
        assert!(row(&buffer, 0).contains("independent scales"));
        assert!(row(&buffer, 3).contains("Cache reads"));
        assert!(row(&buffer, 3).contains("Reuse 99%"));
        assert_eq!(row(&buffer, 1), row(&buffer, 4));
        assert_eq!(row(&buffer, 2), row(&buffer, 5));
        assert!(row(&buffer, 1)
            .chars()
            .any(|c| c > '\u{2800}' && c <= '\u{28ff}'));
        assert_ne!(buffer[(58, 1)].fg, buffer[(58, 4)].fg);
    }

    #[test]
    fn compact_cache_summaries_preserve_metrics_and_original_chart() {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        app.show_cache_read_rate = true;
        let now = Instant::now();
        app.record_global_token_delta(
            now,
            TokenRates {
                input: 200,
                output: 100,
                cache_read: 600,
                cache_write: 200,
            },
        );
        for height in 1..=3 {
            let buffer = render(&app, height, now);
            let text = (0..height)
                .map(|y| row(&buffer, y))
                .collect::<Vec<_>>()
                .join("\n");
            for metric in ["500/min", "600/min", "Reuse 60%"] {
                assert!(
                    text.contains(metric),
                    "{height} rows missing {metric}: {text}"
                );
            }
            assert!(!text.contains("independent scales"));
            if height == 3 {
                assert!(row(&buffer, 1)
                    .chars()
                    .any(|c| c > '\u{2800}' && c <= '\u{28ff}'));
                assert!(row(&buffer, 2).contains("Cache reads"));
            }
        }
        app.show_cache_read_rate = false;
        let buffer = render(&app, 6, now);
        let text = (0..6)
            .map(|y| row(&buffer, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !text.contains("Cache")
                && !text.contains("Reuse")
                && !text.contains("independent scales")
        );
        assert!(row(&buffer, 5)
            .chars()
            .any(|c| c > '\u{2800}' && c <= '\u{28ff}'));
    }

    #[test]
    fn summary_expires_by_elapsed_time_without_erasing_graph_history() {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        app.show_cache_read_rate = true;
        let now = Instant::now();
        app.record_global_token_delta(
            now,
            TokenRates {
                output: 100,
                ..Default::default()
            },
        );
        let buffer = render(&app, 2, now);
        assert!(row(&buffer, 0).contains("100/min"));
        assert!(row(&buffer, 1).contains("0/min") && row(&buffer, 1).contains("Reuse —"));
        let buffer = render(&app, 6, now + Duration::from_secs(60));
        assert!(row(&buffer, 0).contains("0/min"));
        assert!(row(&buffer, 3).contains("0/min") && row(&buffer, 3).contains("Reuse —"));
        assert_eq!(app.token_rates.back(), Some(&100.0));
        app.clear_global_token_history();
        let buffer = render(&app, 2, now);
        assert!(row(&buffer, 1).contains("0/min") && row(&buffer, 1).contains("Reuse —"));
    }
}
