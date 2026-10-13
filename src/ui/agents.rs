use crate::app::{App, StatsScope};
use crate::theme::Theme;
use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::time::Instant;

use super::{braille_graph_multirow, grad_at, make_gradient};

pub(crate) fn scope_area(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(2),
        area.y,
        area.width.saturating_sub(6).min(30),
        area.height.min(1),
    )
}

pub(crate) fn draw(f: &mut Frame, app: &App, area: Rect, theme: &Theme, active: bool) {
    let title = format!("agents · g {}", app.agent_scope.label());
    let block = super::btop_block_active(&title, "⁰", theme.cpu_box, theme, active);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    if app.agent_scope == StatsScope::Selected {
        let message = match app.sessions.get(app.selected) {
            None => Some("No session selected"),
            Some(s) if !matches!(s.agent_cli, "claude" | "codex") => {
                Some("Agent activity unavailable")
            }
            _ => None,
        };
        if let Some(message) = message {
            f.render_widget(
                Paragraph::new(message).style(Style::default().fg(theme.inactive_fg)),
                inner,
            );
            return;
        }
    }
    let now = Instant::now();
    let counts = app.displayed_agent_counts();
    let graph_width = inner.width.saturating_sub(2) as usize;
    // Match Token rate's two time samples per braille character.
    let bars = app.agent_bars(graph_width * 2, now);
    let claude_gradient =
        make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);
    let codex_gradient = make_gradient(
        theme.proc_grad.start,
        theme.proc_grad.mid,
        theme.proc_grad.end,
    );
    let peak = bars
        .iter()
        .flatten()
        .map(|b| b.working())
        .max()
        .unwrap_or(0);
    let mut summary = vec![
        Span::styled(
            format!("Now {}  ", counts.working()),
            Style::default().fg(theme.main_fg),
        ),
        Span::styled(
            format!("Claude {}  ", counts.claude.working),
            Style::default().fg(grad_at(&claude_gradient, 50.0)),
        ),
        Span::styled(
            format!("Codex {}  ", counts.codex.working),
            Style::default().fg(grad_at(&codex_gradient, 50.0)),
        ),
        Span::raw(format!("Peak {peak}")),
    ];
    if counts.unknown() > 0 {
        summary.push(Span::styled(
            format!(" · Unknown {}", counts.unknown()),
            Style::default().fg(theme.inactive_fg),
        ));
    }
    let mut lines = vec![Line::from(summary)];
    let height = inner.height.saturating_sub(2) as usize;
    let max = f64::from(peak.max(1));
    let normalized: Vec<f64> = bars
        .iter()
        .map(|bar| bar.map_or(0.0, |bar| f64::from(bar.working()) / max))
        .collect();
    let mut graph = braille_graph_multirow(
        &normalized,
        graph_width,
        height,
        &claude_gradient,
        theme.graph_text,
    );
    for (row_index, row) in graph.iter_mut().enumerate() {
        let bottom = (height - row_index - 1) * 4;
        let top = bottom + 4;
        for (column, span) in row.iter_mut().enumerate() {
            let pair = &bars[column * 2..column * 2 + 2];
            // A terminal cell has one foreground color. Color cells containing
            // upper-stack dots with Codex's gradient; retain Claude below it.
            let contains_codex = pair.iter().flatten().any(|bar| {
                let total = (f64::from(bar.working()) / max * (height * 4) as f64).round() as usize;
                let claude =
                    (f64::from(bar.claude.working) / max * (height * 4) as f64).round() as usize;
                total.min(top) > claude.max(bottom)
            });
            if contains_codex {
                let level = normalized[column * 2].max(normalized[column * 2 + 1]);
                span.style = Style::default().fg(grad_at(&codex_gradient, level * 100.0));
            }
        }
        let mut spans = vec![Span::raw(" ")];
        spans.append(row);
        lines.push(Line::from(spans));
    }
    if inner.height >= 2 {
        let middle = inner.width.saturating_sub(9) as usize;
        lines.push(Line::styled(
            format!(" -15m{}now ", " ".repeat(middle)),
            Style::default().fg(theme.inactive_fg),
        ));
    }
    f.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme.graph_text)),
        inner,
    );
}
