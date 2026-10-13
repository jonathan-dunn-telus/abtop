use crate::app::App;
use crate::locale::t;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::{
    braille_sparkline, btop_block_active, fmt_tokens, grad_at, make_gradient, meter_bar,
    styled_label, truncate_str,
};

/// Keep the cache-read exclusion visible even when bars must disappear.
fn cache_read_label(inner_width: usize, value_width: usize) -> String {
    let label = t("tokens.cache_r");
    [
        format!(" {label} excluded:"),
        format!(" {label} excl:"),
        "CR excl".to_string(),
    ]
    .into_iter()
    .find(|label| label.width() + 1 + value_width <= inner_width)
    .unwrap_or_else(|| "CR excl".to_string())
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
    let total = selected.map(|s| s.total_tokens()).unwrap_or(0);
    let raw_total = total + cache_read;
    let turns: u32 = selected.map(|s| s.turn_count).unwrap_or(0);
    let avg = if turns > 0 { total / turns as u64 } else { 0 };

    // Category bars show shares of all raw usage, including excluded cache reads.
    let (in_pct, out_pct, cache_r_pct, cache_w_pct) = if raw_total > 0 {
        (
            total_in as f64 / raw_total as f64 * 100.0,
            total_out as f64 / raw_total as f64 * 100.0,
            cache_read as f64 / raw_total as f64 * 100.0,
            cache_write as f64 / raw_total as f64 * 100.0,
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

    let inner_width = area.width.saturating_sub(2) as usize;
    let rows = [
        (
            format!(" {} :", t("tokens.input")),
            total_in,
            in_pct,
            &free_grad,
        ),
        (
            format!(" {}:", t("tokens.output")),
            total_out,
            out_pct,
            &used_grad,
        ),
        (
            cache_read_label(inner_width, fmt_tokens(cache_read).width()),
            cache_read,
            cache_r_pct,
            &cached_grad,
        ),
        (
            format!(" {}:", t("tokens.cache_w")),
            cache_write,
            cache_w_pct,
            &cached_grad,
        ),
    ];
    let text_width = rows
        .iter()
        .map(|(label, value, _, _)| label.width() + 1 + fmt_tokens(*value).width())
        .max()
        .unwrap_or(0);
    let bar_width = inner_width.saturating_sub(text_width).min(15);
    let [input_line, output_line, cache_r_line, cache_w_line] =
        rows.map(|(label, value, percentage, gradient)| {
            let mut line = vec![styled_label(&label, theme.graph_text)];
            line.extend(meter_bar(percentage, bar_width, gradient, theme.meter_bg));
            line.push(Span::styled(
                format!(" {}", fmt_tokens(value)),
                Style::default().fg(grad_at(gradient, 80.0)),
            ));
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
    use ratatui::{backend::TestBackend, Terminal};

    fn render(totals: [u64; 4], turns: u32, width: u16) -> Vec<String> {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        crate::demo::populate_demo(&mut app);
        app.selected = 0;
        let session = &mut app.sessions[0];
        [
            session.total_input_tokens,
            session.total_output_tokens,
            session.total_cache_read,
            session.total_cache_create,
        ] = totals;
        session.turn_count = turns;
        let mut terminal = Terminal::new(TestBackend::new(width, 9)).unwrap();
        terminal
            .draw(|f| draw_tokens_panel(f, &app, f.area(), &app.theme))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..9)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn tokens_exclude_reads_and_keep_the_exclusion_and_values_visible() {
        for width in [15, 20, 32, 60] {
            let lines = render([100, 50, 200, 30], 2, width);
            assert!(lines[1].contains("180"), "{width}: {}", lines[1]);
            assert!(lines[4].contains("200"), "{width}: {}", lines[4]);
            assert!(lines[4].contains("excl"), "{width}: {}", lines[4]);
            assert!(lines[5].contains("30"), "{width}: {}", lines[5]);
            if width >= 32 {
                assert!(lines[7].contains("90/t"));
            }
        }
    }

    #[test]
    fn cache_only_and_zero_turn_usage_stay_visible_without_invalid_averages() {
        for totals in [[0, 0, 200, 0], [0; 4]] {
            let lines = render(totals, 0, 32);
            assert!(lines[1].contains("Total: 0"));
            assert!(lines[4].contains("excl"));
            assert!(lines[7].contains("0/t"));
            assert!(!lines.join("\n").contains("NaN"));
        }
    }
}
