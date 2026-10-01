use crate::app::App;
use crate::locale::t;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{braille_graph_multirow, btop_block_active, fmt_tokens, grad_at, make_gradient};

pub(crate) fn draw_token_rate_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    let gradient = make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);
    let block = btop_block_active("token rate", "⁹", theme.cpu_box, theme, active);
    let inner = block.inner(area);
    f.render_widget(block, area);
    draw_token_rate_chart(f, app, inner, &gradient, theme);
}

fn draw_token_rate_chart(
    f: &mut Frame,
    app: &App,
    area: Rect,
    cpu_grad: &[Color; 101],
    theme: &Theme,
) {
    if area.is_empty() {
        return;
    }
    let avail_h = area.height as usize;
    let avail_w = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    let spark_w = avail_w.saturating_sub(2).max(4);
    let rates: Vec<f64> = app.token_rates.iter().copied().collect();
    let max_rate = rates.iter().cloned().fold(1.0_f64, f64::max);
    let normalized: Vec<f64> = rates.iter().map(|&v| v / max_rate).collect();

    // Graph title with current rate (btop-style)
    let ticks_per_min = 30usize;
    let tokens_per_min: f64 = rates.iter().rev().take(ticks_per_min).sum();
    let current_pct = normalized.last().copied().unwrap_or(0.0) * 100.0;
    let pct_color = grad_at(cpu_grad, current_pct);
    let token_rate_label = t("token_rate.global");
    lines.push(Line::from(vec![
        Span::styled(token_rate_label, Style::default().fg(theme.graph_text)),
        Span::styled(
            format!("  {}/min", fmt_tokens(tokens_per_min as u64)),
            Style::default().fg(pct_color),
        ),
    ]));

    // Multi-row braille area graph (fills available height minus the rate label)
    let graph_h = avail_h.saturating_sub(1);
    let graph_rows =
        braille_graph_multirow(&normalized, spark_w, graph_h, cpu_grad, theme.graph_text);
    for row_spans in graph_rows {
        let mut line_spans = vec![Span::styled(" ", Style::default())];
        line_spans.extend(row_spans);
        lines.push(Line::from(line_spans));
    }

    f.render_widget(Paragraph::new(lines), area);
}
