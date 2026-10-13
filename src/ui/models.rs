use crate::app::App;
use crate::locale::t;
use crate::model::ModelUsage;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

use super::{btop_block_active, fmt_tokens, grad_at, make_gradient, truncate_str};

#[derive(Debug, PartialEq, Eq)]
struct ModelRow {
    usage: ModelUsage,
    rate: u64,
}

/// Keep the largest models visible and sum both totals and rates for overflow.
fn visible_usage(
    entries: &[ModelUsage],
    capacity: usize,
    rate: &impl Fn(&str) -> u64,
) -> Vec<ModelRow> {
    if capacity == 0 {
        return Vec::new();
    }
    let mut entries: Vec<_> = ModelUsage::sorted(entries)
        .into_iter()
        .map(|usage| ModelRow {
            rate: rate(&usage.model),
            usage,
        })
        .collect();
    if entries.len() > capacity {
        let remaining = entries.split_off(capacity - 1);
        entries.push(ModelRow {
            usage: ModelUsage {
                model: format!("{} ({})", t("models.other"), remaining.len()),
                total_tokens: remaining.iter().map(|entry| entry.usage.total_tokens).sum(),
                active_tokens: remaining
                    .iter()
                    .map(|entry| entry.usage.active_tokens)
                    .sum(),
            },
            rate: remaining.iter().map(|entry| entry.rate).sum(),
        });
    }
    entries
}

pub(crate) fn draw_models_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    let block = btop_block_active(&t("models.title"), "⁸", theme.mem_box, theme, active);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    let selected = app.sessions.get(app.selected);
    let Some(session) = selected.filter(|session| session.total_tokens() > 0) else {
        let message = if selected.is_none() {
            t("models.no_session")
        } else {
            t("models.no_usage")
        };
        f.render_widget(
            Paragraph::new(message).style(Style::default().fg(theme.graph_text)),
            inner,
        );
        return;
    };

    let cpu_grad = make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);
    let width = inner.width as usize;
    let now = Instant::now();
    let rate = |model: &str| app.model_token_rate(session, model, now);
    // Measure complete entries, including the aggregated Other row: a rate
    // needs a third line only when it cannot fit after the percentage.
    let mut capacity = (inner.height as usize)
        .min(session.model_usage.len())
        .max(1);
    let lines = loop {
        let entries = visible_usage(&session.model_usage, capacity, &rate);
        let mut lines = Vec::new();
        for entry in entries {
            let share = entry.usage.total_tokens as f64 / session.total_tokens() as f64 * 100.0;
            let totals = format!("{} {:>5.1}%", fmt_tokens(entry.usage.total_tokens), share);
            let rate = format!("{}/min", fmt_tokens(entry.rate));
            let metrics = format!("{totals} {rate}");
            let metrics_style = Style::default().fg(grad_at(&cpu_grad, share));
            // A single remaining content row still gets a compact summary.
            if inner.height >= 2 && (width < 36 || metrics.width() + 1 >= width) {
                lines.push(Line::from(Span::styled(
                    truncate_str(&entry.usage.model, width),
                    Style::default().fg(theme.main_fg),
                )));
                if metrics.width() <= width {
                    lines.push(Line::from(Span::styled(metrics, metrics_style)));
                } else {
                    lines.push(Line::from(Span::styled(totals, metrics_style)));
                    lines.push(Line::from(Span::styled(rate, metrics_style)));
                }
            } else {
                let name_width = width.saturating_sub(metrics.width() + 1);
                let name = truncate_str(&entry.usage.model, name_width);
                let padding = width.saturating_sub(name.width() + metrics.width());
                lines.push(Line::from(vec![
                    Span::styled(name, Style::default().fg(theme.main_fg)),
                    Span::raw(" ".repeat(padding)),
                    Span::styled(metrics, metrics_style),
                ]));
            }
        }
        if lines.len() <= inner.height as usize || capacity == 1 {
            break lines;
        }
        capacity -= 1;
    };
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn usage(model: &str, total_tokens: u64) -> ModelUsage {
        ModelUsage {
            model: model.to_string(),
            total_tokens,
            active_tokens: total_tokens,
        }
    }

    fn app_with_usage() -> App {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        crate::demo::populate_demo(&mut app);
        let session = &mut app.sessions[0];
        session.total_input_tokens = 1_000;
        session.total_output_tokens = 0;
        session.total_cache_read = 0;
        session.total_cache_create = 0;
        session.model_usage = vec![usage("model-b", 250), usage("model-a", 750)];
        app.selected = 0;
        app
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw_models_panel_active(f, app, f.area(), &app.theme, false))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn overflow_preserves_totals_and_ties_sort_by_identity() {
        let entries = vec![
            usage("b", 100),
            usage("a", 100),
            usage("c", 20),
            usage("d", 10),
        ];
        assert_eq!(
            visible_usage(&entries, 3, &|_| 5),
            vec![
                ModelRow {
                    usage: usage("a", 100),
                    rate: 5
                },
                ModelRow {
                    usage: usage("b", 100),
                    rate: 5
                },
                ModelRow {
                    usage: usage("Other (2)", 30),
                    rate: 10
                },
            ]
        );
        assert_eq!(
            visible_usage(&entries, 1, &|_| 5),
            vec![ModelRow {
                usage: usage("Other (4)", 230),
                rate: 20,
            }]
        );
        assert!(visible_usage(&entries, 0, &|_| 5).is_empty());
    }

    #[test]
    fn renders_totals_shares_and_selected_session_in_both_layouts() {
        let mut app = app_with_usage();
        for width in [20, 50] {
            let text = render(&app, width, 6);
            assert!(text.contains("model-a"), "{text}");
            assert!(text.contains("750  75.0%"), "{text}");
            assert!(text.contains("250  25.0%"), "{text}");
            assert!(text.find("model-a") < text.find("model-b"));
        }
        app.selected = 1;
        let text = render(&app, 50, 6);
        assert!(!text.contains("model-a"));
        assert!(text.contains("100.0%"), "{text}");
    }

    #[test]
    fn rates_follow_percentages_and_wrap_only_when_metrics_do_not_fit() {
        let mut app = app_with_usage();
        let session = &mut app.sessions[0];
        session.model_usage = vec![
            usage("claude-opus-5-5", 174_187_700),
            usage("claude-sonnet-5-5", 4_799_300),
            usage("gpt-6-astra", 77_600),
        ];
        session.total_input_tokens = session
            .model_usage
            .iter()
            .map(|entry| entry.total_tokens)
            .sum();
        let now = Instant::now();
        app.update_session_token_rates(now - std::time::Duration::from_secs(2));
        let session = &mut app.sessions[0];
        for (entry, delta) in session.model_usage.iter_mut().zip([12_300, 700, 0]) {
            entry.total_tokens += delta;
            entry.active_tokens += delta;
            session.total_input_tokens += delta;
        }
        app.update_session_token_rates(now);

        // The screenshot-sized pane keeps the rate on the existing second line.
        let text = render(&app, 34, 8);
        let lines: Vec<_> = text.lines().collect();
        assert!(lines[1].contains("claude-opus-5-5"), "{text}");
        assert!(lines[2].contains("174.2M  97.3% 12.3k/min"), "{text}");
        assert!(lines[4].contains("4.8M   2.7% 700/min"), "{text}");
        assert!(lines[6].contains("77.6k   0.0% 0/min"), "{text}");

        // Wide entries put everything on one line.
        let text = render(&app, 70, 5);
        assert!(
            text.lines()
                .any(|line| line.contains("claude-opus-5-5")
                    && line.contains("174.2M  97.3% 12.3k/min")),
            "{text}"
        );

        // At the unchanged minimum width, wrap the rate and measure overflow
        // using the actual row heights, including Other's aggregated rate.
        let text = render(&app, 20, 8);
        let lines: Vec<_> = text.lines().collect();
        assert!(lines[2].contains("174.2M  97.3%"), "{text}");
        assert!(lines[3].contains("12.3k/min"), "{text}");
        assert!(lines[4].contains("Other (2)"), "{text}");
        assert!(lines[6].contains("700/min"), "{text}");

        let text = render(&app, 80, 3);
        assert!(text.contains("Other (3)"), "{text}");
        assert!(text.contains("100.0% 13k/min"), "{text}");
        app.selected = 1;
        assert!(!render(&app, 70, 5).contains("12.3k/min"));
    }

    #[test]
    fn empty_states_and_unknown_usage_are_explicit() {
        let mut app = app_with_usage();
        app.sessions[0].model_usage = vec![usage("Unknown", 1_000)];
        assert!(render(&app, 40, 6).contains("Unknown"));
        app.sessions[0].total_input_tokens = 0;
        assert!(render(&app, 40, 6).contains("No usage yet"));
        app.sessions.clear();
        assert!(render(&app, 40, 6).contains("No session selected"));
    }

    #[test]
    fn narrow_overflow_and_wide_characters_fit_without_losing_metrics() {
        let mut app = app_with_usage();
        let session = &mut app.sessions[0];
        session.model_usage = vec![
            usage("model-\u{ff21}\u{ff22}-very-long-name", 750),
            usage("Unknown", 250),
        ];
        let text = render(&app, 20, 6);
        assert!(text.contains('…'), "{text}");
        assert!(text.contains("75.0%"), "{text}");
        let text = render(&app, 20, 4);
        assert!(text.contains("Other (2)"), "{text}");
        assert!(text.contains("100.0%"), "{text}");
        for (width, height) in [(1, 1), (2, 2), (16, 3)] {
            render(&app, width, height);
        }
    }
}
