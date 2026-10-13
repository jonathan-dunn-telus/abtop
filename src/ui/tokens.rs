use crate::app::App;
use crate::locale::t;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

use super::{
    braille_sparkline, btop_block_active, fmt_tokens, grad_at, make_gradient, meter_bar,
    styled_label, truncate_str,
};

struct TokenRow<'a> {
    label: String,
    total_text: String,
    rate_text: String,
    percentage: f64,
    gradient: &'a [Color; 101],
}

impl<'a> TokenRow<'a> {
    fn new(
        label: String,
        total: u64,
        rate: u64,
        percentage: f64,
        gradient: &'a [Color; 101],
    ) -> Self {
        Self {
            label,
            total_text: format!(" {}", fmt_tokens(total)),
            rate_text: format!("  {}/min", fmt_tokens(rate)),
            percentage,
            gradient,
        }
    }
}

pub(crate) fn draw_tokens_panel(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    draw_tokens_panel_active(f, app, area, theme, false);
}

pub(crate) fn draw_tokens_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    let selected = app.sessions.get(app.selected);
    let total_in: u64 = selected.map(|s| s.total_input_tokens).unwrap_or(0);
    let total_out: u64 = selected.map(|s| s.total_output_tokens).unwrap_or(0);
    let cache_read: u64 = selected.map(|s| s.total_cache_read).unwrap_or(0);
    let cache_write: u64 = selected.map(|s| s.total_cache_create).unwrap_or(0);
    let total: u64 = total_in + total_out + cache_read + cache_write;
    let turns: u32 = selected.map(|s| s.turn_count).unwrap_or(0);
    let avg = if turns > 0 { total / turns as u64 } else { 0 };

    // Compute percentages for mini meter bars
    let (in_pct, out_pct, cache_r_pct, cache_w_pct) = if total > 0 {
        (
            total_in as f64 / total as f64 * 100.0,
            total_out as f64 / total as f64 * 100.0,
            cache_read as f64 / total as f64 * 100.0,
            cache_write as f64 / total as f64 * 100.0,
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };

    let free_grad = make_gradient(
        theme.free_grad.start,
        theme.free_grad.mid,
        theme.free_grad.end,
    );
    let used_grad = make_gradient(
        theme.used_grad.start,
        theme.used_grad.mid,
        theme.used_grad.end,
    );
    let cached_grad = make_gradient(
        theme.cached_grad.start,
        theme.cached_grad.mid,
        theme.cached_grad.end,
    );

    let total_label = t("tokens.total");
    let total_line = vec![
        styled_label(format!(" {}: ", total_label).as_str(), theme.graph_text),
        Span::styled(
            fmt_tokens(total),
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    let now = Instant::now();
    let rates = selected
        .map(|session| app.session_token_rates(session, now))
        .unwrap_or_default();
    let rows = [
        TokenRow::new(
            format!(" {} :", t("tokens.input")),
            total_in,
            rates.input,
            in_pct,
            &free_grad,
        ),
        TokenRow::new(
            format!(" {}:", t("tokens.output")),
            total_out,
            rates.output,
            out_pct,
            &used_grad,
        ),
        TokenRow::new(
            format!(" {}:", t("tokens.cache_r")),
            cache_read,
            rates.cache_read,
            cache_r_pct,
            &cached_grad,
        ),
        TokenRow::new(
            format!(" {}:", t("tokens.cache_w")),
            cache_write,
            rates.cache_write,
            cache_w_pct,
            &cached_grad,
        ),
    ];

    // Reserve text space before allocating bars, using the same bar width and
    // rate visibility for every row. Account for the panel's two borders.
    let inner_w = area.width.saturating_sub(2) as usize;
    let rate_text_w = rows
        .iter()
        .map(|row| row.label.width() + row.total_text.width() + row.rate_text.width())
        .max()
        .unwrap_or(0);
    let show_rates = rate_text_w <= inner_w;
    let text_w = if show_rates {
        rate_text_w
    } else {
        rows.iter()
            .map(|row| row.label.width() + row.total_text.width())
            .max()
            .unwrap_or(0)
    };
    let bar_w = inner_w.saturating_sub(text_w).min(15);
    let [input_line, output_line, cache_r_line, cache_w_line] = rows.map(|row| {
        let mut line = vec![styled_label(&row.label, theme.graph_text)];
        line.extend(meter_bar(row.percentage, bar_w, row.gradient, theme.meter_bg));
        line.push(Span::styled(
            row.total_text,
            Style::default().fg(grad_at(row.gradient, 80.0)),
        ));
        if show_rates {
            line.push(Span::styled(
                row.rate_text,
                Style::default().fg(theme.graph_text),
            ));
        }
        line
    });

    // Per-turn sparkline from selected session's token_history
    let cpu_grad = make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);
    let all_history: Vec<u64> = app
        .sessions
        .get(app.selected)
        .map(|s| s.token_history.clone())
        .unwrap_or_default();
    let spark_w = (area.width as usize).saturating_sub(16).clamp(5, 20);
    let max_val = all_history.iter().copied().max().unwrap_or(1).max(1);
    let normalized: Vec<f64> = all_history
        .iter()
        .map(|&v| v as f64 / max_val as f64)
        .collect();
    let mut spark_line_spans = vec![styled_label(" ", theme.graph_text)];
    spark_line_spans.extend(braille_sparkline(
        &normalized,
        spark_w,
        &cpu_grad,
        theme.graph_text,
    ));
    let tokens_turn_label = t("tokens.tokens_turn");
    spark_line_spans.push(Span::styled(
        format!(" {}", tokens_turn_label),
        Style::default().fg(theme.graph_text),
    ));

    let turns_label = t("tokens.turns");
    let avg_label = t("tokens.avg");
    let lines = vec![
        Line::from(total_line),
        Line::from(input_line),
        Line::from(output_line),
        Line::from(cache_r_line),
        Line::from(cache_w_line),
        Line::from(spark_line_spans),
        Line::from(vec![
            styled_label(format!(" {}: ", turns_label).as_str(), theme.graph_text),
            Span::styled(format!("{}", turns), Style::default().fg(theme.main_fg)),
            styled_label(format!("  {}: ", avg_label).as_str(), theme.graph_text),
            Span::styled(
                format!("{}/t", fmt_tokens(avg)),
                Style::default().fg(theme.graph_text),
            ),
        ]),
    ];

    let panel_title = if let Some(s) = selected {
        format!(
            "tokens ({}/{})",
            truncate_str(&s.project_name, 12),
            truncate_str(&s.session_id, 8)
        )
    } else {
        "tokens".to_string()
    };
    let block = btop_block_active(&panel_title, "³", theme.mem_box, theme, active);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::time::Duration;

    fn app_with_rates(totals: [u64; 4], rates: [u64; 4]) -> App {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        crate::demo::populate_demo(&mut app);
        app.sessions.truncate(1);
        app.selected = 0;
        let set_totals = |app: &mut App, values: [u64; 4]| {
            let session = &mut app.sessions[0];
            session.total_input_tokens = values[0];
            session.total_output_tokens = values[1];
            session.total_cache_read = values[2];
            session.total_cache_create = values[3];
        };
        let now = Instant::now();
        set_totals(&mut app, std::array::from_fn(|i| totals[i] - rates[i]));
        app.update_session_token_rates(now - Duration::from_secs(10));
        set_totals(&mut app, totals);
        app.update_session_token_rates(now - Duration::from_secs(1));
        app
    }

    fn render(app: &App, width: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, 9)).unwrap();
        terminal
            .draw(|f| draw_tokens_panel(f, app, f.area(), &app.theme))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..9)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn category_rates_follow_totals_and_bars_yield_space_first() {
        let app = app_with_rates(
            [402_000, 89_000, 710_000, 12_000],
            [12_300, 4_500, 6_800, 0],
        );
        for width in [60, 32, 27, 26] {
            let lines = render(&app, width);
            for (row, label, values) in [
                (2, "Input", "402k  12.3k/min"),
                (3, "Output", "89k  4.5k/min"),
                (4, "CacheR", "710k  6.8k/min"),
                (5, "CacheW", "12k  0/min"),
            ] {
                assert!(lines[row].contains(label), "{}", lines[row]);
                assert!(lines[row].contains(values), "{width}: {}", lines[row]);
                assert_eq!(lines[row].contains('■'), width > 26);
            }
            assert!(!lines[1].contains("/min"), "Total row has no rate");
        }
    }

    #[test]
    fn narrow_pane_omits_all_rates_while_preserving_totals() {
        let app = app_with_rates(
            [402_000, 89_000, 710_000, 12_000],
            [12_300, 4_500, 6_800, 0],
        );
        for width in [25, 20, 15] {
            let lines = render(&app, width);
            assert!(!lines.join("\n").contains("/min"));
            for (row, total) in [(2, "402k"), (3, "89k"), (4, "710k"), (5, "12k")] {
                assert!(lines[row].contains(total), "{width}: {}", lines[row]);
            }
        }
    }

    #[test]
    fn large_totals_and_rates_fit_without_clipping() {
        let app = app_with_rates([1_234_567_890; 4], [1_000_000_000; 4]);
        let lines = render(&app, 36);
        for line in &lines[2..6] {
            assert!(line.contains("1234.6M  1000M/min"), "{line}");
        }
    }

    #[test]
    fn empty_selection_displays_zero_totals_and_rates() {
        let app = App::new_with_config(Theme::default(), &[], Default::default());
        let lines = render(&app, 32);
        for line in &lines[2..6] {
            assert!(line.contains(" 0  0/min"), "{line}");
        }
    }
}
