use crate::app::{App, StatsScope};
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

use super::{btop_block_active, fmt_tokens, stats_panel_title, truncate_str};

/// Keep the largest models visible and account for all remaining usage in one row.
fn visible_usage(entries: &[ModelUsage], capacity: usize) -> Vec<ModelUsage> {
    let mut entries = ModelUsage::sorted(entries);
    if capacity == 0 {
        return Vec::new();
    }
    if entries.len() > capacity {
        let remaining = entries.split_off(capacity - 1);
        entries.push(ModelUsage {
            model: format!("{} ({})", t("models.other"), remaining.len()),
            total_tokens: remaining.iter().map(|entry| entry.total_tokens).sum(),
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
    let title = stats_panel_title(app, &t("models.title"), area.width, active);
    let block = btop_block_active(&title, "⁸", theme.mem_box, theme, active);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    let stats = app.usage_stats(Instant::now());
    if stats.total_tokens() == 0 {
        let message = if stats.session_count == 0 && app.stats_scope == StatsScope::AllLive {
            "No live sessions".to_string()
        } else if stats.session_count == 0 {
            t("models.no_session")
        } else {
            t("models.no_usage")
        };
        f.render_widget(
            Paragraph::new(message).style(Style::default().fg(theme.graph_text)),
            inner,
        );
        return;
    }

    let width = inner.width as usize;
    // A single remaining content row still gets a compact summary.
    let two_lines = width < 36 && inner.height >= 2;
    let row_height = if two_lines { 2 } else { 1 };
    let entries = visible_usage(&stats.model_usage, inner.height as usize / row_height);
    let mut lines = Vec::new();
    for entry in entries {
        let share = entry.total_tokens as f64 / stats.total_tokens() as f64 * 100.0;
        let metrics = format!("{} {:>5.1}%", fmt_tokens(entry.total_tokens), share);
        if two_lines {
            lines.push(Line::from(Span::styled(
                truncate_str(&entry.model, width),
                Style::default().fg(theme.main_fg),
            )));
            lines.push(Line::from(Span::styled(
                metrics,
                Style::default().fg(theme.title),
            )));
        } else {
            let name_width = width.saturating_sub(metrics.width() + 1);
            let name = truncate_str(&entry.model, name_width);
            let padding = width.saturating_sub(name.width() + metrics.width());
            lines.push(Line::from(vec![
                Span::styled(name, Style::default().fg(theme.main_fg)),
                Span::raw(" ".repeat(padding)),
                Span::styled(metrics, Style::default().fg(theme.title)),
            ]));
        }
    }
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
    fn aggregate_models_use_combined_denominator_and_explicit_scope() {
        let mut app = app_with_usage();
        app.sessions.truncate(1);
        let mut other = app.sessions[0].clone();
        other.session_id = "other-session".into();
        other.total_input_tokens = 500;
        other.model_usage = vec![usage("model-a", 500)];
        app.sessions.push(other);
        app.stats_scope = StatsScope::AllLive;
        for width in [20, 40, 80] {
            let text = render(&app, width, 10);
            assert!(text.contains("All live"), "{text}");
            assert!(text.contains("83.3%"), "{text}");
            assert!(text.contains("16.7%"), "{text}");
        }
        app.sessions.clear();
        assert!(render(&app, 40, 6).contains("No live sessions"));
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
            visible_usage(&entries, 3),
            vec![usage("a", 100), usage("b", 100), usage("Other (2)", 30)]
        );
        assert_eq!(visible_usage(&entries, 1), vec![usage("Other (4)", 230)]);
        assert!(visible_usage(&entries, 0).is_empty());
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
