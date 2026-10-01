mod config;
mod context;
mod footer;
mod header;
mod help;
mod mcp;
mod models;
mod ports;
mod projects;
mod quota;
mod sessions;
mod token_rate;
mod tokens;
mod view_menu;

use crate::app::{App, NarrowSection, NarrowTab, StatsScope};
use crate::locale::t;
use crate::theme::Theme;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ── braille graph symbols — from btop_draw.cpp ──────────────────────────────
// 5x5 lookup: [prev_val * 5 + cur_val], values 0-4
pub(crate) const BRAILLE_UP: [&str; 25] = [
    " ", "⢀", "⢠", "⢰", "⢸", "⡀", "⣀", "⣠", "⣰", "⣸", "⡄", "⣄", "⣤", "⣴", "⣼", "⡆", "⣆", "⣦", "⣶",
    "⣾", "⡇", "⣇", "⣧", "⣷", "⣿",
];

// ── gradient interpolation (btop-faithful: linear RGB, 101 steps) ────────────

/// Generate 101-step gradient from start→mid→end, matching btop's generateGradients().
pub(crate) fn make_gradient(
    start: (u8, u8, u8),
    mid: (u8, u8, u8),
    end: (u8, u8, u8),
) -> [Color; 101] {
    let mut out = [Color::Reset; 101];
    #[allow(clippy::needless_range_loop)]
    for i in 0..=100 {
        let (s, e, offset, range) = if i <= 50 {
            (start, mid, 0, 50)
        } else {
            (mid, end, 50, 50)
        };
        let t = i - offset;
        let r = s.0 as i32 + t as i32 * (e.0 as i32 - s.0 as i32) / range;
        let g = s.1 as i32 + t as i32 * (e.1 as i32 - s.1 as i32) / range;
        let b = s.2 as i32 + t as i32 * (e.2 as i32 - s.2 as i32) / range;
        out[i] = Color::Rgb(
            r.clamp(0, 255) as u8,
            g.clamp(0, 255) as u8,
            b.clamp(0, 255) as u8,
        );
    }
    out
}

/// Pick color from a gradient at a given percentage.
pub(crate) fn grad_at(gradient: &[Color; 101], pct: f64) -> Color {
    let idx = (pct.clamp(0.0, 100.0)).round() as usize;
    gradient[idx.min(100)]
}

// ── btop-style meter bar using ■ character ───────────────────────────────────

/// Render a btop-style meter: filled ■ with gradient color, empty ■ with meter_bg.
pub(crate) fn meter_bar(
    pct: f64,
    width: usize,
    gradient: &[Color; 101],
    meter_bg: Color,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let clamped = pct.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f64).round() as usize;
    let mut spans = Vec::new();
    for i in 0..width {
        if i < filled {
            let cell_pct = (i as f64 / width as f64) * 100.0;
            spans.push(Span::styled(
                "■",
                Style::default().fg(grad_at(gradient, cell_pct)),
            ));
        } else {
            spans.push(Span::styled("■", Style::default().fg(meter_bg)));
        }
    }
    spans
}

/// Meter bar showing remaining quota: filled = remaining, color reflects urgency.
/// When remaining is high → green (safe), when low → red (danger).
pub(crate) fn remaining_bar(
    remaining_pct: f64,
    width: usize,
    gradient: &[Color; 101],
    meter_bg: Color,
) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let clamped = remaining_pct.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f64).round() as usize;
    let used_pct = 100.0 - clamped;
    let mut spans = Vec::new();
    for i in 0..width {
        if i < filled {
            let cell_pct = used_pct;
            spans.push(Span::styled(
                "■",
                Style::default().fg(grad_at(gradient, cell_pct)),
            ));
        } else {
            spans.push(Span::styled("■", Style::default().fg(meter_bg)));
        }
    }
    spans
}

// ── braille sparkline ────────────────────────────────────────────────────────

/// Render a braille sparkline from data points (0.0–1.0), colored with gradient.
pub(crate) fn braille_sparkline(
    data: &[f64],
    width: usize,
    gradient: &[Color; 101],
    graph_text: Color,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if data.is_empty() || width == 0 {
        for _ in 0..width {
            spans.push(Span::styled(" ", Style::default().fg(graph_text)));
        }
        return spans;
    }

    // We need pairs of data points per braille char (prev, cur)
    // Pad or sample data to fit width * 2 points
    let needed = width * 2;
    let sampled: Vec<f64> = if data.len() >= needed {
        data[data.len() - needed..].to_vec()
    } else {
        let mut v = vec![0.0; needed - data.len()];
        v.extend_from_slice(data);
        v
    };

    for i in 0..width {
        let prev = (sampled[i * 2].clamp(0.0, 1.0) * 4.0).round() as usize;
        let cur = (sampled[i * 2 + 1].clamp(0.0, 1.0) * 4.0).round() as usize;
        let idx = prev * 5 + cur;
        let pct = (sampled[i * 2 + 1] * 100.0).round() as usize;
        let color = grad_at(gradient, pct as f64);
        spans.push(Span::styled(
            BRAILLE_UP[idx.min(24)].to_string(),
            Style::default().fg(color),
        ));
    }
    spans
}

// ── multi-row braille area graph (btop-style filled CPU graph) ──────────────

/// Render a multi-row braille area graph. `data` values are 0.0–1.0.
/// Returns one Vec<Span> per terminal row (top to bottom).
pub(crate) fn braille_graph_multirow(
    data: &[f64],
    width: usize,
    height: usize,
    gradient: &[Color; 101],
    graph_text: Color,
) -> Vec<Vec<Span<'static>>> {
    if height == 0 || width == 0 {
        return vec![vec![]; height];
    }

    let total_vres = height * 4; // vertical resolution in braille dots
    let needed = width * 2; // 2 data points per braille character

    let sampled: Vec<f64> = if data.len() >= needed {
        data[data.len() - needed..].to_vec()
    } else {
        let mut v = vec![0.0; needed - data.len()];
        v.extend_from_slice(data);
        v
    };

    let heights: Vec<usize> = sampled
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * total_vres as f64).round() as usize)
        .collect();

    // Braille dot bits — bottom-to-top within each cell:
    // Left col:  row0(bottom)=0x40, row1=0x04, row2=0x02, row3(top)=0x01
    // Right col: row0(bottom)=0x80, row1=0x20, row2=0x10, row3(top)=0x08
    let left_bits: [u32; 4] = [0x40, 0x04, 0x02, 0x01];
    let right_bits: [u32; 4] = [0x80, 0x20, 0x10, 0x08];

    let mut rows: Vec<Vec<Span<'static>>> = Vec::with_capacity(height);

    for row in 0..height {
        let mut spans = Vec::with_capacity(width);
        let inv_row = height - 1 - row; // row 0 in output = top of graph
        let base_y = inv_row * 4;

        for col in 0..width {
            let left_h = heights[col * 2];
            let right_h = heights[col * 2 + 1];

            let mut pattern: u32 = 0;
            for dot_row in 0..4u32 {
                let y_pos = base_y + dot_row as usize;
                if left_h > y_pos {
                    pattern |= left_bits[dot_row as usize];
                }
                if right_h > y_pos {
                    pattern |= right_bits[dot_row as usize];
                }
            }

            let ch = char::from_u32(0x2800 + pattern).unwrap_or(' ');
            let max_val = sampled[col * 2].max(sampled[col * 2 + 1]);
            let color = if pattern == 0 {
                graph_text
            } else {
                grad_at(gradient, max_val * 100.0)
            };
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
        }
        rows.push(spans);
    }

    rows
}

// ── btop-style block with notch title: ──┐¹title┌────── ─────────────────────

pub(crate) fn btop_block_active(
    title: &str,
    number: &str,
    box_color: Color,
    theme: &Theme,
    active: bool,
) -> Block<'static> {
    let title = if active {
        format!("{title}(*)")
    } else {
        title.to_string()
    };
    Block::default()
        .title(Line::from(vec![
            Span::styled("┐", Style::default().fg(box_color)),
            Span::styled(
                number.to_string(),
                Style::default()
                    .fg(theme.hi_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                title,
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("┌", Style::default().fg(box_color)),
        ]))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(box_color))
}

pub(crate) fn styled_label(text: &str, graph_text: Color) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().fg(graph_text))
}

/// Keep scope readable before allocating the rest of a narrow pane's title.
pub(crate) fn stats_panel_title(app: &App, name: &str, width: u16, active: bool) -> String {
    let available = width.saturating_sub(5 + if active { 3 } else { 0 }) as usize;
    let candidates = match app.stats_scope {
        StatsScope::AllLive => vec![
            format!("{name} · All live"),
            format!("{name} All live"),
            "All live".to_string(),
        ],
        StatsScope::Selected => match app.sessions.get(app.selected) {
            Some(session) => vec![
                format!(
                    "{name} ({}/{})",
                    truncate_str(&session.project_name, 12),
                    truncate_str(&session.session_id, 8)
                ),
                format!("{name} {}", truncate_str(&session.session_id, 8)),
                format!(
                    "{name} {}",
                    truncate_str(
                        &session.session_id,
                        available.saturating_sub(name.width() + 1)
                    )
                ),
            ],
            None => vec![name.to_string()],
        },
    };
    candidates
        .iter()
        .find(|title| title.width() <= available)
        .cloned()
        .unwrap_or_else(|| truncate_str(candidates.last().unwrap(), available))
}

// ── main draw ────────────────────────────────────────────────────────────────

const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 18;
const DESKTOP_MIN_WIDTH: u16 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClickTarget {
    StatsScope,
    NarrowTab(NarrowTab),
    NarrowSection(NarrowSection),
    NarrowZoom(NarrowSection),
    Session(usize),
    KillOrphanPorts,
}

struct DesktopLayout {
    header: Rect,
    context: Option<Rect>,
    token_rate: Option<Rect>,
    mid: Vec<(NarrowSection, Rect)>,
    sessions: Option<Rect>,
    footer: Rect,
}

pub fn draw(f: &mut Frame, app: &App) {
    let theme = &app.theme;
    let area = f.area();
    let w = area.width;
    let h = area.height;

    // Paint the entire frame with the theme's background so the app renders
    // correctly on light-background terminals (where text would otherwise be
    // light-on-light and unreadable).
    f.render_widget(
        Block::default().style(Style::default().bg(theme.main_bg).fg(theme.main_fg)),
        area,
    );

    if w < MIN_WIDTH || h < MIN_HEIGHT {
        let msg = vec![
            Line::from(Span::styled(
                t("term.too_small"),
                Style::default()
                    .fg(theme.main_fg)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(
                    format!("{} ", t("term.width")),
                    Style::default().fg(theme.main_fg),
                ),
                Span::styled(
                    w.to_string(),
                    Style::default().fg(if w < MIN_WIDTH {
                        Color::Red
                    } else {
                        Color::Green
                    }),
                ),
                Span::styled(
                    format!(" {} ", t("term.height")),
                    Style::default().fg(theme.main_fg),
                ),
                Span::styled(
                    h.to_string(),
                    Style::default().fg(if h < MIN_HEIGHT {
                        Color::Red
                    } else {
                        Color::Green
                    }),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                t("term.needed"),
                Style::default()
                    .fg(theme.main_fg)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                format!(
                    "{} {}  {} {}",
                    t("term.width"),
                    MIN_WIDTH,
                    t("term.height"),
                    MIN_HEIGHT
                ),
                Style::default().fg(theme.main_fg),
            )),
        ];
        let block = Paragraph::new(msg).alignment(Alignment::Center);
        let y = h / 2 - 2;
        let msg_area = Rect {
            x: 0,
            y,
            width: w,
            height: 5.min(h.saturating_sub(y)),
        };
        f.render_widget(block, msg_area);
        return;
    }

    if !uses_desktop_layout(app, w) {
        draw_narrow(f, app, area, theme);
        draw_overlays(f, app, theme);
        return;
    }

    let layout = desktop_layout(app, area);
    header::draw_header(f, app, layout.header, theme);

    if let Some(area) = layout.token_rate {
        token_rate::draw_token_rate_panel_active(f, app, area, theme, false);
    }

    if let Some(area) = layout.context {
        context::draw_context_panel(f, app, area, theme);
    }

    for (section, area) in layout.mid {
        match section {
            NarrowSection::Quota => quota::draw_quota_panel(f, app, area, theme),
            NarrowSection::Tokens => tokens::draw_tokens_panel(f, app, area, theme),
            NarrowSection::Models => models::draw_models_panel_active(f, app, area, theme, false),
            NarrowSection::Projects => projects::draw_projects_panel(f, app, area, theme),
            NarrowSection::Ports => ports::draw_ports_panel(f, app, area, theme),
            NarrowSection::Mcp => mcp::draw_mcp_panel(f, app, area, theme),
            NarrowSection::Sessions | NarrowSection::Context | NarrowSection::TokenRate => {}
        }
    }

    if let Some(area) = layout.sessions {
        sessions::draw_sessions_panel(f, app, area, theme);
    }
    footer::draw_footer(f, app, layout.footer, theme);

    draw_overlays(f, app, theme);
}

fn desktop_sections(app: &App) -> Vec<NarrowSection> {
    let mut mid_sections = Vec::new();
    if app.show_quota {
        mid_sections.push(NarrowSection::Quota);
    }
    if app.show_tokens {
        mid_sections.push(NarrowSection::Tokens);
    }
    if app.show_models {
        mid_sections.push(NarrowSection::Models);
    }
    if app.show_projects {
        mid_sections.push(NarrowSection::Projects);
    }
    if app.show_ports {
        mid_sections.push(NarrowSection::Ports);
    }
    if app.show_mcp {
        mid_sections.push(NarrowSection::Mcp);
    }
    mid_sections
}

fn desktop_min_width(section: NarrowSection) -> u16 {
    match section {
        NarrowSection::Quota | NarrowSection::Ports | NarrowSection::Mcp => 24,
        NarrowSection::Tokens => 22,
        NarrowSection::Models | NarrowSection::Projects => 20,
        NarrowSection::Sessions | NarrowSection::Context | NarrowSection::TokenRate => 0,
    }
}

fn uses_desktop_layout(app: &App, width: u16) -> bool {
    let required = desktop_sections(app)
        .into_iter()
        .map(desktop_min_width)
        .sum::<u16>()
        .max(DESKTOP_MIN_WIDTH);
    width >= required
}

fn desktop_layout(app: &App, area: Rect) -> DesktopLayout {
    const TOP_MIN: u16 = 5;
    const FIXED: u16 = 2; // header + footer

    let mid_sections = desktop_sections(app);

    let any_mid = !mid_sections.is_empty();
    let mid_h_ideal: u16 = 8;
    let sessions_ideal: u16 = if app.show_sessions {
        (app.sessions.len() as u16 * 2 + 7).max(8)
    } else {
        0
    };
    let top_min_ideal = if app.show_token_rate && app.show_cache_read_rate {
        6
    } else {
        5
    };
    let top_ideal: u16 = (app.sessions.len() as u16 + 4).clamp(top_min_ideal, 10);

    let available = area.height.saturating_sub(FIXED);
    // Tokens need five content rows for the total and all four categories.
    let mid_min = if app.show_tokens { 7 } else { 6 };
    let mid_reserved = if any_mid { mid_min.min(available) } else { 0 };
    let sessions_budget = available.saturating_sub(mid_reserved);
    let sessions_h = if app.show_sessions {
        sessions_ideal
            .min(sessions_budget)
            .max(5.min(sessions_budget))
    } else {
        0
    };
    let after_sessions = available.saturating_sub(sessions_h);
    let mid_h = if any_mid {
        mid_h_ideal
            .min(after_sessions)
            .max(mid_reserved.min(after_sessions))
    } else {
        0
    };
    let surplus = available.saturating_sub(sessions_h + mid_h);
    let top_enabled = app.show_context || app.show_token_rate;
    let top_h = if top_enabled && sessions_h >= sessions_ideal && surplus >= TOP_MIN {
        top_ideal.min(surplus)
    } else {
        0
    };

    let mut constraints = [Constraint::Length(0); 5];
    let mut n = 0;
    constraints[n] = Constraint::Length(1);
    n += 1;
    if top_h > 0 {
        constraints[n] = Constraint::Length(top_h);
        n += 1;
    }
    if mid_h > 0 {
        constraints[n] = Constraint::Length(mid_h);
        n += 1;
    }
    if sessions_h > 0 {
        constraints[n] = Constraint::Min(sessions_h);
        n += 1;
    }
    constraints[n] = Constraint::Length(1);
    n += 1;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(&constraints[..n])
        .split(area);

    let mut idx = 0;
    let header = chunks[idx];
    idx += 1;

    let (token_rate, context) = if top_h > 0 {
        let area = chunks[idx];
        idx += 1;
        match (app.show_token_rate, app.show_context) {
            (true, true) => {
                let panes =
                    Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
                        .split(area);
                (Some(panes[0]), Some(panes[1]))
            }
            (true, false) => (Some(area), None),
            (false, true) => (None, Some(area)),
            (false, false) => (None, None),
        }
    } else {
        (None, None)
    };

    let mid = if mid_h > 0 {
        let minima: Vec<_> = mid_sections
            .iter()
            .copied()
            .map(desktop_min_width)
            .collect();
        let mid_panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(minimum_constraints(&minima, chunks[idx].width))
            .split(chunks[idx]);
        idx += 1;
        mid_sections
            .into_iter()
            .zip(mid_panels.iter().copied())
            .collect()
    } else {
        Vec::new()
    };

    let sessions = if sessions_h > 0 {
        let area = chunks[idx];
        idx += 1;
        Some(area)
    } else {
        None
    };

    DesktopLayout {
        header,
        context,
        token_rate,
        mid,
        sessions,
        footer: chunks[idx],
    }
}

fn draw_narrow(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    header::draw_header(f, app, chunks[0], theme);

    let body = chunks[1];
    draw_active_narrow_panel(f, app, body, theme);

    draw_narrow_tabs(f, app, chunks[2], theme);
    footer::draw_footer(f, app, chunks[3], theme);
}

fn draw_narrow_tabs(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let active = app.active_narrow_tab();
    let tab_areas = narrow_tab_layout(app, area);
    let used = narrow_tab_group_width(&tab_areas);
    let pad = area.width.saturating_sub(used) as usize;
    let mut spans: Vec<Span> = Vec::new();
    if pad > 0 {
        spans.push(Span::styled(
            " ".repeat(pad),
            Style::default().bg(theme.main_bg),
        ));
    }
    for (i, (tab, _)) in tab_areas.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default().bg(theme.main_bg)));
        }
        let selected = Some(tab) == active;
        let mut style = Style::default()
            .bg(if selected {
                theme.selected_bg
            } else {
                theme.main_bg
            })
            .fg(if selected {
                theme.selected_fg
            } else {
                theme.inactive_fg
            });
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        };
        spans.push(Span::styled(narrow_tab_label(tab), style));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.main_bg)),
        area,
    );
}

fn narrow_tab_label(tab: NarrowTab) -> String {
    format!(" {}({}) ", tab.label(), tab.shortcut())
}

fn narrow_tab_width(tab: NarrowTab) -> u16 {
    narrow_tab_label(tab).chars().count() as u16
}

fn narrow_tab_group_width(tab_areas: &[(NarrowTab, Rect)]) -> u16 {
    let labels = tab_areas.iter().map(|(_, area)| area.width).sum::<u16>();
    labels + tab_areas.len().saturating_sub(1) as u16
}

fn draw_active_narrow_panel(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let Some(tab) = app.active_narrow_tab() else {
        return;
    };

    let sections = narrow_section_areas(app, tab, area);
    let active = visible_active_section(app, &sections);
    for (section, section_area) in sections {
        draw_narrow_section(
            f,
            app,
            section_area,
            theme,
            section,
            active == Some(section),
        );
        draw_narrow_zoom_button(f, app, section_area, theme, section);
    }
}

fn narrow_section_areas(app: &App, tab: NarrowTab, area: Rect) -> Vec<(NarrowSection, Rect)> {
    let mut sections = if let Some(section) = app.maximized_narrow_section() {
        if section.tab() == tab {
            vec![section]
        } else {
            app.visible_narrow_sections(tab)
        }
    } else {
        app.visible_narrow_sections(tab)
    };
    if sections.is_empty() {
        return Vec::new();
    }

    if sections.len() == 1 {
        return vec![(sections[0], area)];
    }

    let constraints = if tab == NarrowTab::Usage {
        let min_height = |section| match section {
            NarrowSection::Context | NarrowSection::TokenRate => 5,
            NarrowSection::Quota => 6,
            NarrowSection::Tokens => 7,
            NarrowSection::Models => 4,
            _ => 0,
        };
        // Optional graphs yield before the core usage rows, without changing
        // saved visibility. Keep context usage available longer than token rate.
        for optional in [NarrowSection::TokenRate, NarrowSection::Context] {
            if sections.iter().copied().map(min_height).sum::<u16>() > area.height {
                sections.retain(|section| *section != optional);
            }
        }
        let mut minima: Vec<_> = sections.iter().copied().map(min_height).collect();
        if minima.iter().sum::<u16>() > area.height {
            // At the minimum terminal height, omit quota's summary and show
            // one Models row (with overflow) to preserve every token category.
            for (section, minimum) in sections.iter().zip(&mut minima) {
                match section {
                    NarrowSection::Quota => *minimum = 5,
                    NarrowSection::Models => *minimum = 3,
                    _ => {}
                }
            }
        }
        minimum_constraints(&minima, area.height)
    } else {
        let count = sections.len() as u32;
        (0..count).map(|_| Constraint::Ratio(1, count)).collect()
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    sections.into_iter().zip(chunks.iter().copied()).collect()
}

/// Reserve each pane's essential content, then share spare space evenly.
fn minimum_constraints(minima: &[u16], available: u16) -> Vec<Constraint> {
    let count = minima.len() as u16;
    let extra = available.saturating_sub(minima.iter().sum());
    minima
        .iter()
        .enumerate()
        .map(|(index, minimum)| {
            Constraint::Length(minimum + extra / count + u16::from((index as u16) < extra % count))
        })
        .collect()
}

fn visible_active_section(app: &App, sections: &[(NarrowSection, Rect)]) -> Option<NarrowSection> {
    app.active_narrow_section()
        .filter(|active| sections.iter().any(|(section, _)| section == active))
        .or_else(|| sections.first().map(|(section, _)| *section))
}

/// Keep keyboard zoom on a visible pane when resizing hides optional panes.
pub(crate) fn sync_narrow_section(app: &mut App, area: Rect) {
    if uses_desktop_layout(app, area.width) || area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return;
    }
    if let Some(tab) = app.active_narrow_tab() {
        let sections = narrow_section_areas(app, tab, narrow_chunks(area)[1]);
        if let Some(active) = visible_active_section(app, &sections) {
            app.set_active_narrow_section(active);
        }
    }
}

fn draw_narrow_section(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    section: NarrowSection,
    active: bool,
) {
    match section {
        NarrowSection::Sessions => {
            sessions::draw_sessions_panel_active(f, app, area, theme, active)
        }
        NarrowSection::Projects => {
            projects::draw_projects_panel_active(f, app, area, theme, active)
        }
        NarrowSection::Context => context::draw_context_panel_active(f, app, area, theme, active),
        NarrowSection::TokenRate => {
            token_rate::draw_token_rate_panel_active(f, app, area, theme, active)
        }
        NarrowSection::Quota => quota::draw_quota_panel_active(f, app, area, theme, active),
        NarrowSection::Tokens => tokens::draw_tokens_panel_active(f, app, area, theme, active),
        NarrowSection::Models => models::draw_models_panel_active(f, app, area, theme, active),
        NarrowSection::Ports => ports::draw_ports_panel_active(f, app, area, theme, active),
        NarrowSection::Mcp => mcp::draw_mcp_panel_active(f, app, area, theme, active),
    }
}

fn draw_narrow_zoom_button(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    section: NarrowSection,
) {
    let Some(button_area) = zoom_button_area(area) else {
        return;
    };
    let label = if app.maximized_narrow_section() == Some(section) {
        " - "
    } else {
        " + "
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            label,
            Style::default()
                .bg(theme.main_bg)
                .fg(theme.hi_fg)
                .add_modifier(Modifier::BOLD),
        ))),
        button_area,
    );
}

fn draw_overlays(f: &mut Frame, app: &App, theme: &Theme) {
    if app.config_open {
        config::draw_config_overlay(f, app, theme);
    }
    if app.view_open {
        view_menu::draw_view_overlay(f, app, theme);
    }
    if app.help_open {
        help::draw_help_overlay(f, theme);
    }
}

pub(crate) fn click_target(app: &App, area: Rect, column: u16, row: u16) -> Option<ClickTarget> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }
    if contains(header::stats_control_area(app, area), column, row) {
        return Some(ClickTarget::StatsScope);
    }
    if uses_desktop_layout(app, area.width) {
        let layout = desktop_layout(app, area);
        if let Some(sessions_area) = layout.sessions {
            if contains(sessions_area, column, row) {
                return session_at(app, sessions_area, row).map(ClickTarget::Session);
            }
        }
        for (section, section_area) in layout.mid {
            if section == NarrowSection::Ports
                && contains(section_area, column, row)
                && ports_kill_at(app, section_area, row)
            {
                return Some(ClickTarget::KillOrphanPorts);
            }
        }
        return None;
    }

    let chunks = narrow_chunks(area);
    if contains(chunks[2], column, row) {
        return narrow_tab_at(app, chunks[2], column).map(ClickTarget::NarrowTab);
    }

    let tab = app.active_narrow_tab()?;

    if contains(chunks[1], column, row) {
        for (section, section_area) in narrow_section_areas(app, tab, chunks[1]) {
            if !contains(section_area, column, row) {
                continue;
            }
            if zoom_button_at(section_area, column, row) {
                return Some(ClickTarget::NarrowZoom(section));
            }
            if section == NarrowSection::Sessions {
                if let Some(index) = session_at(app, section_area, row) {
                    return Some(ClickTarget::Session(index));
                }
            }
            if section == NarrowSection::Ports && ports_kill_at(app, section_area, row) {
                return Some(ClickTarget::KillOrphanPorts);
            }
            return Some(ClickTarget::NarrowSection(section));
        }
    }

    None
}

fn zoom_button_area(area: Rect) -> Option<Rect> {
    if area.width < 5 || area.height == 0 {
        return None;
    }
    Some(Rect {
        x: area.x + area.width - 4,
        y: area.y,
        width: 3,
        height: 1,
    })
}

fn zoom_button_at(area: Rect, column: u16, row: u16) -> bool {
    zoom_button_area(area).is_some_and(|button| contains(button, column, row))
}

fn narrow_chunks(area: Rect) -> Vec<Rect> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area)
        .to_vec()
}

fn narrow_tab_at(app: &App, area: Rect, column: u16) -> Option<NarrowTab> {
    for (tab, tab_area) in narrow_tab_layout(app, area) {
        if contains(tab_area, column, area.y) {
            return Some(tab);
        }
    }
    None
}

fn narrow_tab_layout(app: &App, area: Rect) -> Vec<(NarrowTab, Rect)> {
    let tabs = app.visible_narrow_tabs();
    if tabs.is_empty() {
        return Vec::new();
    }
    let labels_width = tabs.iter().map(|&tab| narrow_tab_width(tab)).sum::<u16>();
    let gaps = tabs.len().saturating_sub(1) as u16;
    let total = labels_width.saturating_add(gaps).min(area.width);
    let mut x = area.x + area.width.saturating_sub(total);
    let mut out = Vec::with_capacity(tabs.len());
    for (i, tab) in tabs.into_iter().enumerate() {
        if i > 0 {
            x = x.saturating_add(1);
        }
        let width = narrow_tab_width(tab).min(area.x + area.width - x);
        if width == 0 {
            break;
        }
        out.push((
            tab,
            Rect {
                x,
                y: area.y,
                width,
                height: 1,
            },
        ));
        x = x.saturating_add(width);
        if x >= area.x + area.width {
            break;
        }
    }
    out
}

fn session_at(app: &App, area: Rect, row: u16) -> Option<usize> {
    if area.height < 4 || row <= area.y + 1 {
        return None;
    }

    let inner_h = area.height.saturating_sub(2);
    let visible = app.visible_indices();
    let session_rows: u16 = visible
        .iter()
        .map(|&i| {
            let base = 2u16;
            if app.tree_view {
                base + app.sessions[i].subagents.len() as u16
            } else {
                base
            }
        })
        .sum();
    let detail_reserve: u16 = if app.show_timeline {
        (inner_h * 2 / 3).min(inner_h.saturating_sub(5))
    } else if inner_h <= 12 {
        6.min(inner_h.saturating_sub(3))
    } else {
        10.min(inner_h / 2)
    };
    let max_table = inner_h.saturating_sub(detail_reserve);
    let table_h = (1 + session_rows).min(max_table);
    let table_y = area.y + 1;
    if row >= table_y.saturating_add(table_h) {
        return None;
    }

    let visible_rows = table_h.saturating_sub(1) as usize;
    let selected_pos = visible.iter().position(|&i| i == app.selected).unwrap_or(0);
    let selected_row_start: usize = visible
        .iter()
        .take(selected_pos)
        .map(|&i| {
            let base = 2;
            if app.tree_view {
                base + app.sessions[i].subagents.len()
            } else {
                base
            }
        })
        .sum();
    let selected_session_rows = if app.tree_view {
        2 + app
            .sessions
            .get(app.selected)
            .map_or(0, |s| s.subagents.len())
    } else {
        2
    };
    let scroll_offset = (selected_row_start + selected_session_rows).saturating_sub(visible_rows);
    let target_row = scroll_offset + row.saturating_sub(table_y + 1) as usize;
    let mut offset = 0usize;
    for &idx in &visible {
        let rows = if app.tree_view {
            2 + app.sessions[idx].subagents.len()
        } else {
            2
        };
        if target_row >= offset && target_row < offset + rows {
            return Some(idx);
        }
        offset += rows;
    }

    None
}

fn ports_kill_at(app: &App, area: Rect, row: u16) -> bool {
    if app.orphan_ports.is_empty() || area.height < 3 {
        return false;
    }

    let live_ports = app
        .sessions
        .iter()
        .map(|session| {
            session
                .children
                .iter()
                .filter(|child| child.port.is_some())
                .count()
        })
        .sum::<usize>() as u16;
    let kill_line = 1 + live_ports + app.orphan_ports.len() as u16;
    let kill_row = area.y + 1 + kill_line;
    row == kill_row && kill_row < area.y + area.height.saturating_sub(1)
}

fn contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

// ── utility functions ────────────────────────────────────────────────────────

pub(crate) fn fmt_mem_kb(kb: u64) -> String {
    if kb >= 1_048_576 {
        format!("{:.1}G", kb as f64 / 1_048_576.0)
    } else if kb >= 1024 {
        format!("{}M", kb / 1024)
    } else {
        format!("{}K", kb)
    }
}

pub(crate) fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        let v = n as f64 / 1_000_000.0;
        if v == v.floor() {
            format!("{}M", v as u64)
        } else {
            format!("{:.1}M", v)
        }
    } else if n >= 1_000 {
        let v = n as f64 / 1_000.0;
        if v == v.floor() {
            format!("{}k", v as u64)
        } else {
            format!("{:.1}k", v)
        }
    } else {
        format!("{}", n)
    }
}

/// Format a duration in seconds into a compact "Ns / Nm / Nh / Nd ago" label.
pub(crate) fn fmt_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}{}", secs, t("time.s_ago"))
    } else if secs < 3600 {
        format!("{}{}", secs / 60, t("time.m_ago"))
    } else if secs < 86400 {
        format!("{}{}", secs / 3600, t("time.h_ago"))
    } else {
        format!("{}{}", secs / 86400, t("time.d_ago"))
    }
}

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(s) <= max {
        s.to_string()
    } else {
        let mut width = 0;
        let mut truncated = String::new();
        for ch in s.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width >= max {
                break;
            }
            truncated.push(ch);
            width += ch_width;
        }
        format!("{}…", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PanelVisibility;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn fmt_age_buckets() {
        // t() defaults to English when ABTOP_LANG is unset, so the strings
        // here match the en-US locale values for `time.{s,m,h,d}_ago`.
        assert_eq!(fmt_age(5), "5s ago");
        assert_eq!(fmt_age(59), "59s ago");
        assert_eq!(fmt_age(60), "1m ago");
        assert_eq!(fmt_age(125), "2m ago");
        assert_eq!(fmt_age(7_200), "2h ago");
        // Regression: the quota panel used to render this raw as "341493ago"
        // because it formatted seconds without unit conversion.
        assert_eq!(fmt_age(341_493), "3d ago");
    }

    #[test]
    fn truncate_str_respects_terminal_display_width() {
        assert_eq!(truncate_str("ＡＢ123", 6), "ＡＢ1…");
    }

    #[test]
    fn compact_sizes_render_sessions_instead_of_too_small() {
        for (w, h) in [(69, 27), (80, 24)] {
            let text = render_demo(w, h);
            assert!(text.contains("Work"), "{w}x{h} should render tabs\n{text}");
            assert!(
                text.contains("Usage"),
                "{w}x{h} should expose grouped panels as tabs\n{text}"
            );
            assert!(
                text.contains("System(s)"),
                "{w}x{h} should render system tab shortcut\n{text}"
            );
            assert!(
                text.contains("sessions"),
                "{w}x{h} should render sessions panel\n{text}"
            );
            assert!(
                text.contains("sessions(*)"),
                "{w}x{h} should mark the active section in the title\n{text}"
            );
            assert!(
                text.contains("projects"),
                "{w}x{h} should pair sessions with projects\n{text}"
            );
            assert!(
                text.contains("SESSION"),
                "{w}x{h} should render selected-session detail\n{text}"
            );
            assert!(
                !text.contains("Terminal size too small"),
                "{w}x{h} should be supported\n{text}"
            );
            assert!(
                !text.contains("quota"),
                "{w}x{h} should not spend first screen on mid panels\n{text}"
            );
        }
    }

    #[test]
    fn compact_tab_switch_renders_selected_panel() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.set_narrow_tab(NarrowTab::Usage);

        let backend = TestBackend::new(69, 27);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = format!("{}", terminal.backend());

        assert!(
            text.contains("quota"),
            "usage tab should render quota panel\n{text}"
        );
        assert!(
            text.contains("tokens"),
            "usage tab should render tokens panel\n{text}"
        );
        assert!(
            !text.contains("SESSION"),
            "usage tab should not keep sessions detail in body\n{text}"
        );
    }

    #[test]
    fn compact_click_targets_tabs_and_sessions() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        let area = Rect {
            x: 0,
            y: 0,
            width: 69,
            height: 27,
        };
        let chunks = narrow_chunks(area);
        let tab_area = chunks[2];
        let tab_areas = narrow_tab_layout(&app, tab_area);
        let usage_area = tab_areas
            .iter()
            .find(|(tab, _)| *tab == NarrowTab::Usage)
            .map(|(_, area)| *area)
            .unwrap();
        let separator_x = usage_area.x - 1;

        assert_eq!(
            click_target(&app, area, usage_area.x, tab_area.y),
            Some(ClickTarget::NarrowTab(NarrowTab::Usage))
        );
        assert_eq!(click_target(&app, area, separator_x, tab_area.y), None);
        assert_eq!(click_target(&app, area, 3, tab_area.y), None);
        assert_eq!(
            click_target(&app, area, 3, 4),
            Some(ClickTarget::Session(0))
        );

        assert_eq!(
            click_target(&app, area, 3, 16),
            Some(ClickTarget::NarrowSection(NarrowSection::Projects))
        );

        let sessions_area = narrow_section_areas(&app, NarrowTab::Work, chunks[1])
            .into_iter()
            .find(|(section, _)| *section == NarrowSection::Sessions)
            .map(|(_, area)| area)
            .unwrap();
        assert_eq!(
            click_target(
                &app,
                area,
                sessions_area.x + sessions_area.width - 3,
                sessions_area.y
            ),
            Some(ClickTarget::NarrowZoom(NarrowSection::Sessions))
        );
    }

    #[test]
    fn compact_tabs_highlight_only_active_tab() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        let area = Rect {
            x: 0,
            y: 0,
            width: 69,
            height: 27,
        };
        let tab_area = narrow_chunks(area)[2];
        let tab_areas = narrow_tab_layout(&app, tab_area);

        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let work = tab_areas
            .iter()
            .find(|(tab, _)| *tab == NarrowTab::Work)
            .map(|(_, area)| *area)
            .unwrap();
        let usage = tab_areas
            .iter()
            .find(|(tab, _)| *tab == NarrowTab::Usage)
            .map(|(_, area)| *area)
            .unwrap();
        let system = tab_areas
            .iter()
            .find(|(tab, _)| *tab == NarrowTab::System)
            .map(|(_, area)| *area)
            .unwrap();

        assert_eq!(
            buffer.cell((work.x, work.y)).unwrap().bg,
            app.theme.selected_bg
        );
        assert_eq!(
            buffer.cell((usage.x, usage.y)).unwrap().bg,
            app.theme.main_bg
        );
        assert_eq!(
            buffer.cell((system.x, system.y)).unwrap().bg,
            app.theme.main_bg
        );
        assert_eq!(
            buffer.cell((work.x, work.y)).unwrap().fg,
            app.theme.selected_fg
        );
        assert_eq!(
            buffer.cell((usage.x, usage.y)).unwrap().fg,
            app.theme.inactive_fg
        );
        assert_eq!(
            buffer.cell((usage.x - 1, usage.y)).unwrap().bg,
            app.theme.main_bg
        );
        assert_eq!(
            buffer.cell((system.x - 1, system.y)).unwrap().bg,
            app.theme.main_bg
        );
    }

    #[test]
    fn compact_usage_reserves_core_rows_and_ports_kill_is_clickable() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        let area = Rect {
            x: 0,
            y: 0,
            width: 69,
            height: 27,
        };
        let body = narrow_chunks(area)[1];

        let usage_sections = narrow_section_areas(&app, NarrowTab::Usage, body);
        assert_eq!(usage_sections.len(), 4);
        for (section, area) in usage_sections {
            let minimum = match section {
                NarrowSection::Context | NarrowSection::TokenRate => 5,
                NarrowSection::Quota => 6,
                NarrowSection::Tokens => 7,
                NarrowSection::Models => 4,
                _ => unreachable!(),
            };
            assert!(area.height >= minimum, "{section:?}: {area:?}");
        }

        app.set_narrow_tab(NarrowTab::System);
        let ports_area = narrow_section_areas(&app, NarrowTab::System, body)
            .into_iter()
            .find(|(section, _)| *section == NarrowSection::Ports)
            .map(|(_, area)| area)
            .unwrap();
        let live_ports = app
            .sessions
            .iter()
            .map(|session| {
                session
                    .children
                    .iter()
                    .filter(|child| child.port.is_some())
                    .count()
            })
            .sum::<usize>() as u16;
        let kill_row = ports_area.y + 1 + 1 + live_ports + app.orphan_ports.len() as u16;
        assert_eq!(
            click_target(&app, area, ports_area.x + 2, kill_row),
            Some(ClickTarget::KillOrphanPorts)
        );
    }

    #[test]
    fn compact_zoom_renders_only_selected_section() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        let area = Rect {
            x: 0,
            y: 0,
            width: 69,
            height: 27,
        };
        let body = narrow_chunks(area)[1];

        app.toggle_narrow_section_zoom(NarrowSection::Quota);
        let sections = narrow_section_areas(&app, NarrowTab::Usage, body);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0], (NarrowSection::Quota, body));

        let backend = TestBackend::new(69, 27);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = format!("{}", terminal.backend());
        assert!(
            text.contains("quota(*)"),
            "zoomed section should stay active\n{text}"
        );
        assert!(
            !text.contains("tokens"),
            "zoomed tab should hide peer sections\n{text}"
        );
    }

    #[test]
    fn stats_scope_remains_visible_in_wide_narrow_and_zoomed_layouts() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.stats_scope = StatsScope::AllLive;
        app.set_narrow_tab(NarrowTab::Usage);
        for (width, height) in [(160, 40), (134, 30), (80, 24), (60, 18)] {
            let area = Rect::new(0, 0, width, height);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();
            let text = format!("{}", terminal.backend());
            assert!(
                text.contains("a Stats: All live ("),
                "{width}x{height}: {text}"
            );
            assert!(
                text.matches("All live").count() >= 3,
                "{width}x{height}: {text}"
            );
            let control = header::stats_control_area(&app, area);
            for column in control.x..control.right() {
                assert_eq!(
                    click_target(&app, area, column, 0),
                    Some(ClickTarget::StatsScope)
                );
            }
        }
        for section in [NarrowSection::Tokens, NarrowSection::Models] {
            app.toggle_narrow_section_zoom(section);
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();
            let text = format!("{}", terminal.backend());
            assert!(text.matches("All live").count() >= 2, "{text}");
            assert!(text.contains("All live(*)"), "{text}");
            app.restore_narrow_sections();
        }
        app.view_open = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = format!("{}", terminal.backend());
        assert!(text.contains("stats scope") && text.contains("All live"));
    }

    #[test]
    fn models_layout_visibility_and_narrow_zoom() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        for (width, height) in [(120, 40), (100, 24), (80, 24), (60, 18)] {
            app.set_narrow_tab(NarrowTab::Usage);
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();
            let text = format!("{}", terminal.backend());
            assert!(text.contains("models"), "{width}x{height}: {text}");
        }
        let desktop = desktop_layout(&app, Rect::new(0, 0, 160, 40));
        let sections: Vec<_> = desktop.mid.iter().map(|(section, _)| *section).collect();
        let tokens = sections
            .iter()
            .position(|section| *section == NarrowSection::Tokens)
            .unwrap();
        assert_eq!(sections[tokens + 1], NarrowSection::Models);

        let area = Rect::new(0, 0, 80, 24);
        let body = narrow_chunks(area)[1];
        let models = narrow_section_areas(&app, NarrowTab::Usage, body)
            .into_iter()
            .find(|(section, _)| *section == NarrowSection::Models)
            .unwrap()
            .1;
        assert_eq!(
            click_target(&app, area, models.x + 1, models.y + 1),
            Some(ClickTarget::NarrowSection(NarrowSection::Models))
        );
        app.toggle_narrow_section_zoom(NarrowSection::Models);
        assert_eq!(
            narrow_section_areas(&app, NarrowTab::Usage, body),
            vec![(NarrowSection::Models, body)]
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = format!("{}", terminal.backend());
        assert!(text.contains("models (") && text.contains("(*)"));
        assert!(text.contains("claude-opus-4-6"));
        assert!(!text.contains("tokens"));
        app.show_models = false;
        assert!(app.maximized_narrow_section().is_none());
        assert!(!app
            .visible_narrow_sections(NarrowTab::Usage)
            .contains(&NarrowSection::Models));
        assert!(!desktop_layout(&app, area)
            .mid
            .iter()
            .any(|(section, _)| *section == NarrowSection::Models));
    }

    #[test]
    fn desktop_click_targets_sessions_and_ports() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        for session in &mut app.sessions {
            session.children.clear();
        }
        let area = Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 40,
        };
        let layout = desktop_layout(&app, area);
        let sessions_area = layout.sessions.unwrap();
        assert_eq!(
            click_target(&app, area, sessions_area.x + 2, sessions_area.y + 2),
            Some(ClickTarget::Session(0))
        );

        let ports_area = layout
            .mid
            .iter()
            .find(|(section, _)| *section == NarrowSection::Ports)
            .map(|(_, area)| *area)
            .unwrap();
        let kill_row = ports_area.y + 1 + 1 + app.orphan_ports.len() as u16;
        assert_eq!(
            click_target(&app, area, ports_area.x + 2, kill_row),
            Some(ClickTarget::KillOrphanPorts)
        );
    }

    #[test]
    fn desktop_default_detail_shows_chat_instead_of_timeline() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.sessions[app.selected].children.clear();
        app.sessions[app.selected].subagents.clear();

        let backend = TestBackend::new(160, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = format!("{}", terminal.backend());

        assert!(
            text.contains("CHAT"),
            "chat should render by default\n{text}"
        );
        assert!(
            text.contains("webhook signatures"),
            "recent chat tail should render selected session messages\n{text}"
        );
        assert!(
            !text.contains("TIMELINE"),
            "timeline should be opt-in via l toggle\n{text}"
        );
    }

    #[test]
    fn desktop_size_keeps_mid_panels() {
        let text = render_demo(160, 40);
        for label in ["quota", "tokens", "models", "projects", "ports", "sessions"] {
            assert!(
                text.contains(label),
                "desktop should render {label}\n{text}"
            );
        }
    }

    fn area_text(buffer: &ratatui::buffer::Buffer, area: Rect) -> String {
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn responsive_usage_keeps_complete_quota_values_and_all_token_categories() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.set_narrow_tab(NarrowTab::Usage);
        for width in [60, 80, 100, 110, 120, 133, 134, 160] {
            for height in [18, 24, 40] {
                let area = Rect::new(0, 0, width, height);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| draw(f, &app)).unwrap();
                let sections = if uses_desktop_layout(&app, width) {
                    desktop_layout(&app, area).mid
                } else {
                    narrow_section_areas(&app, NarrowTab::Usage, narrow_chunks(area)[1])
                };
                for (section, expected) in [
                    (NarrowSection::Quota, vec!["65%", "88%", "91%", "86%"]),
                    (
                        NarrowSection::Tokens,
                        vec![
                            "Total:", "48.2k", "12.8k", "CacheR:", "1.4M", "CacheW:", "185k",
                        ],
                    ),
                    (NarrowSection::Models, vec!["models", "%"]),
                ] {
                    let pane = sections
                        .iter()
                        .find(|(kind, _)| *kind == section)
                        .unwrap()
                        .1;
                    let text = area_text(terminal.backend().buffer(), pane);
                    for value in expected {
                        assert!(
                            text.contains(value),
                            "{width}x{height} {section:?} missing {value}:\n{text}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn responsive_breakpoint_and_mouse_targets_follow_enabled_panes() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        let area = Rect::new(0, 0, 120, 40);
        assert!(!uses_desktop_layout(&app, area.width));
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = area_text(terminal.backend().buffer(), area);
        assert!(text.contains("Usage(u)"));
        assert!(text.contains("←→ tabs"));
        let usage_tab = narrow_tab_layout(&app, narrow_chunks(area)[2])
            .into_iter()
            .find(|(tab, _)| *tab == NarrowTab::Usage)
            .unwrap()
            .1;
        assert_eq!(
            click_target(&app, area, usage_tab.x, usage_tab.y),
            Some(ClickTarget::NarrowTab(NarrowTab::Usage))
        );

        app.show_models = false;
        assert!(uses_desktop_layout(&app, area.width));
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = area_text(terminal.backend().buffer(), area);
        assert!(text.contains("quota"));
        assert!(!text.contains("Usage(u)"));
        let layout = desktop_layout(&app, area);
        for (section, pane) in &layout.mid {
            assert!(pane.width >= desktop_min_width(*section));
        }
        let sessions = layout.sessions.unwrap();
        assert_eq!(
            click_target(&app, area, sessions.x + 2, sessions.y + 2),
            Some(ClickTarget::Session(0))
        );

        app.show_mcp = false;
        assert!(uses_desktop_layout(&app, 100));
        assert!(!uses_desktop_layout(&app, 99));
    }

    #[test]
    fn desktop_top_panes_split_expand_and_yield_without_changing_preferences() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        let area = Rect::new(0, 0, 160, 40);
        let layout = desktop_layout(&app, area);
        let rate = layout.token_rate.unwrap();
        let context = layout.context.unwrap();
        assert_eq!(rate.width, 104);
        assert_eq!(context.width, 56);
        assert_eq!(rate.right(), context.x);
        assert_eq!((rate.y, rate.height), (context.y, context.height));

        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let rate_text = area_text(terminal.backend().buffer(), rate);
        assert!(rate_text.contains("token rate") && rate_text.contains("Global"));
        assert!(rate_text.contains("/min"));
        assert!(!rate_text.contains("Total") && !rate_text.contains("webshop"));
        let context_text = area_text(terminal.backend().buffer(), context);
        for value in ["webshop", "72%", "200k C2", "Window"] {
            assert!(
                context_text.contains(value),
                "missing {value}: {context_text}"
            );
        }
        assert!(!context_text.contains("/min") && !context_text.contains("Total"));

        for (show_rate, show_context) in [(true, false), (false, true), (false, false)] {
            app.show_token_rate = show_rate;
            app.show_context = show_context;
            let layout = desktop_layout(&app, area);
            assert_eq!(layout.token_rate.is_some(), show_rate);
            assert_eq!(layout.context.is_some(), show_context);
            for pane in [layout.token_rate, layout.context].into_iter().flatten() {
                assert_eq!(pane.width, area.width);
                assert_eq!(pane.height, rate.height);
            }
        }
        app.show_token_rate = true;
        app.show_context = true;
        let short = desktop_layout(&app, Rect::new(0, 0, 160, 24));
        assert!(short.token_rate.is_none() && short.context.is_none());
        assert!(app.show_token_rate && app.show_context);
    }

    #[test]
    fn cache_chart_fits_desktop_and_narrow_layouts_without_changing_pane_controls() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.sessions.truncate(1);
        let area = Rect::new(0, 0, 160, 40);
        assert_eq!(desktop_layout(&app, area).token_rate.unwrap().height, 5);
        app.show_cache_read_rate = true;
        assert_eq!(desktop_layout(&app, area).token_rate.unwrap().height, 6);
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        let text = area_text(
            terminal.backend().buffer(),
            desktop_layout(&app, area).token_rate.unwrap(),
        );
        assert!(text.contains("Cache reads") && text.contains("independent scales"));
        // Only five rows are available after reserving Sessions and the middle row.
        let short = desktop_layout(&app, Rect::new(0, 0, 160, 24));
        assert_eq!(short.token_rate.unwrap().height, 5);

        app.set_narrow_tab(NarrowTab::Usage);
        for height in [24, 27, 30, 40] {
            let area = Rect::new(0, 0, 80, height);
            let body = narrow_chunks(area)[1];
            app.show_cache_read_rate = false;
            let without_cache = narrow_section_areas(&app, NarrowTab::Usage, body);
            app.show_cache_read_rate = true;
            assert_eq!(
                narrow_section_areas(&app, NarrowTab::Usage, body),
                without_cache
            );
            sync_narrow_section(&mut app, area);
            app.toggle_narrow_section_zoom(NarrowSection::TokenRate);
            let mut terminal = Terminal::new(TestBackend::new(80, height)).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();
            let text = area_text(terminal.backend().buffer(), body);
            assert!(text.contains("Cache reads") && text.contains("Reuse"));
            assert_eq!(
                click_target(&app, area, body.x + 1, body.y + 1),
                Some(ClickTarget::NarrowSection(NarrowSection::TokenRate))
            );
            assert_eq!(
                click_target(&app, area, body.right() - 3, body.y),
                Some(ClickTarget::NarrowZoom(NarrowSection::TokenRate))
            );
            app.restore_narrow_sections();
        }
        app.view_open = true;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(
            area_text(terminal.backend().buffer(), Rect::new(0, 0, 80, 24))
                .contains("cache-read chart")
        );
        app.view_open = false;
        app.config_open = true;
        terminal.draw(|f| draw(f, &app)).unwrap();
        assert!(
            area_text(terminal.backend().buffer(), Rect::new(0, 0, 80, 24))
                .contains("Cache-read chart (C)")
        );
    }

    #[test]
    fn token_rate_yields_before_context_with_visible_focus_and_independent_zoom() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.set_narrow_tab(NarrowTab::Usage);
        for (height, show_rate, show_context) in
            [(40, true, true), (27, false, true), (24, false, false)]
        {
            let area = Rect::new(0, 0, 80, height);
            let sections = narrow_section_areas(&app, NarrowTab::Usage, narrow_chunks(area)[1]);
            assert_eq!(
                sections.iter().any(|(s, _)| *s == NarrowSection::TokenRate),
                show_rate
            );
            assert_eq!(
                sections.iter().any(|(s, _)| *s == NarrowSection::Context),
                show_context
            );
            assert!(app.show_token_rate && app.show_context);
            sync_narrow_section(&mut app, area);
            let active = app.active_narrow_section().unwrap();
            assert_eq!(active, sections[0].0);
            for (section, pane) in &sections {
                assert_eq!(
                    click_target(&app, area, pane.x + 1, pane.y + 1),
                    Some(ClickTarget::NarrowSection(*section))
                );
                assert_eq!(
                    click_target(&app, area, pane.right() - 3, pane.y),
                    Some(ClickTarget::NarrowZoom(*section))
                );
            }
            app.maximize_active_narrow_section();
            assert_eq!(
                narrow_section_areas(&app, NarrowTab::Usage, narrow_chunks(area)[1]),
                vec![(active, narrow_chunks(area)[1])]
            );
            app.restore_narrow_sections();
        }
        let area = Rect::new(0, 0, 80, 24);
        for section in [NarrowSection::TokenRate, NarrowSection::Context] {
            app.toggle_narrow_section_zoom(section);
            assert_eq!(
                narrow_section_areas(&app, NarrowTab::Usage, narrow_chunks(area)[1]),
                vec![(section, narrow_chunks(area)[1])]
            );
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|f| draw(f, &app)).unwrap();
            let body = area_text(terminal.backend().buffer(), narrow_chunks(area)[1]);
            assert_eq!(body.contains("/min"), section == NarrowSection::TokenRate);
            assert_eq!(body.contains("webshop"), section == NarrowSection::Context);
            app.restore_narrow_sections();
        }
    }

    #[test]
    fn top_panes_keep_their_content_in_compact_and_empty_states() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        for height in [3, 8] {
            let area = Rect::new(0, 0, 80, height);
            let mut terminal = Terminal::new(TestBackend::new(80, height)).unwrap();
            terminal
                .draw(|f| context::draw_context_panel(f, &app, area, &app.theme))
                .unwrap();
            let text = area_text(terminal.backend().buffer(), area);
            assert!(
                text.contains("webshop") && text.contains("72%") && text.contains("200k C2"),
                "{text}"
            );
            assert!(!text.contains("/min") && !text.contains("Total"));
            terminal
                .draw(|f| {
                    token_rate::draw_token_rate_panel_active(f, &app, area, &app.theme, false)
                })
                .unwrap();
            let before = area_text(terminal.backend().buffer(), area);
            app.selected = 1;
            app.stats_scope = StatsScope::AllLive;
            app.filter_text = "no matching session".into();
            terminal
                .draw(|f| {
                    token_rate::draw_token_rate_panel_active(f, &app, area, &app.theme, false)
                })
                .unwrap();
            assert_eq!(before, area_text(terminal.backend().buffer(), area));
        }
        app.sessions.clear();
        app.clear_global_token_history();
        let area = Rect::new(0, 0, 80, 5);
        let mut terminal = Terminal::new(TestBackend::new(80, 5)).unwrap();
        terminal
            .draw(|f| context::draw_context_panel(f, &app, area, &app.theme))
            .unwrap();
        assert!(area_text(terminal.backend().buffer(), area).contains("no active sessions"));
        terminal
            .draw(|f| token_rate::draw_token_rate_panel_active(f, &app, area, &app.theme, false))
            .unwrap();
        assert!(area_text(terminal.backend().buffer(), area).contains("0/min"));
    }

    #[test]
    fn context_yields_to_usage_without_changing_visibility_or_breaking_zoom() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.set_narrow_tab(NarrowTab::Usage);
        let area = Rect::new(0, 0, 80, 24);
        let body = narrow_chunks(area)[1];
        let sections = narrow_section_areas(&app, NarrowTab::Usage, body);
        assert!(!sections
            .iter()
            .any(|(section, _)| *section == NarrowSection::Context));
        assert!(app.show_context);
        for (section, pane) in sections {
            assert_eq!(
                click_target(&app, area, pane.x + 1, pane.y + 1),
                Some(ClickTarget::NarrowSection(section))
            );
        }
        sync_narrow_section(&mut app, area);
        app.maximize_active_narrow_section();
        assert_eq!(
            narrow_section_areas(&app, NarrowTab::Usage, body),
            vec![(NarrowSection::Quota, body)]
        );
        app.restore_narrow_sections();
        assert!(narrow_section_areas(
            &app,
            NarrowTab::Usage,
            narrow_chunks(Rect::new(0, 0, 80, 40))[1]
        )
        .iter()
        .any(|(section, _)| *section == NarrowSection::Context));
        app.toggle_narrow_section_zoom(NarrowSection::Context);
        assert_eq!(
            narrow_section_areas(&app, NarrowTab::Usage, body),
            vec![(NarrowSection::Context, body)]
        );
    }

    fn render_demo(width: u16, height: u16) -> String {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();
        format!("{}", terminal.backend())
    }
}
