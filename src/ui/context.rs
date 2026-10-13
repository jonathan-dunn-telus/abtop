use crate::app::App;
use crate::locale::t;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};
use ratatui::Frame;

use super::{btop_block_active, fmt_tokens, grad_at, make_gradient, meter_bar, truncate_str};

pub(crate) fn draw_context_panel(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    draw_context_panel_active(f, app, area, theme, false);
}

pub(crate) fn draw_context_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    let cpu_grad = make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);

    let block = btop_block_active("context", "¹", theme.cpu_box, theme, active);
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    draw_context_bars(f, app, inner, &cpu_grad, theme);
}

fn draw_context_bars(f: &mut Frame, app: &App, area: Rect, cpu_grad: &[Color; 101], theme: &Theme) {
    if area.is_empty() {
        return;
    }
    if app.sessions.is_empty() {
        f.render_widget(
            Paragraph::new(t("context.no_active_sessions"))
                .style(Style::default().fg(theme.inactive_fg)),
            area,
        );
        return;
    }
    let header_style = Style::default()
        .fg(theme.main_fg)
        .add_modifier(Modifier::BOLD);

    // bar width = remaining space after Project(10) + pct(5) + info(10) + padding
    let bar_width = (area.width as usize).saturating_sub(30).clamp(4, 20);

    let mut rows = Vec::new();

    let project_label = t("context.project");
    let context_label = t("context.context");
    let window_label = t("context.window");

    for session in &app.sessions {
        let raw_pct = session.context_percent;
        let bar_pct = raw_pct.min(100.0);
        let warn = if raw_pct >= 90.0 {
            "⚠"
        } else if raw_pct >= 75.0 {
            "!"
        } else {
            ""
        };
        let pct_color = grad_at(cpu_grad, bar_pct);

        // Context info: window size + compaction count (e.g. "200k C2")
        let ctx_info = match (session.context_window > 0, session.compaction_count) {
            (true, 0) => fmt_tokens(session.context_window),
            (true, n) => format!("{} C{}", fmt_tokens(session.context_window), n),
            (false, 0) => String::new(),
            (false, n) => format!("C{}", n),
        };

        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                truncate_str(&session.project_name, 10),
                Style::default().fg(theme.title),
            )),
            Cell::from(Line::from({
                let mut spans = meter_bar(bar_pct, bar_width, cpu_grad, theme.meter_bg);
                spans.push(Span::styled(
                    format!(" {:>3.0}%{}", raw_pct, warn),
                    Style::default().fg(pct_color),
                ));
                spans
            })),
            Cell::from(Span::styled(
                ctx_info,
                Style::default().fg(theme.graph_text),
            )),
        ]));
    }

    let header = Row::new(vec![
        Cell::from(Span::styled(project_label, header_style)),
        Cell::from(Span::styled(context_label, header_style)),
        Cell::from(Span::styled(window_label, header_style)),
    ]);

    let widths = [
        Constraint::Length(10),
        Constraint::Min(10),
        Constraint::Length(10),
    ];

    let mut table = Table::new(rows, widths);
    if area.height > 1 {
        table = table.header(header);
    }
    f.render_widget(table, area);
}
