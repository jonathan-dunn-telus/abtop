use crate::collector::McpServer;
use crate::host_info::{AgentAggregate, HostMetrics};
use crate::model::{token_total, AgentSession, OrphanPort, RateLimitInfo, SessionStatus};
use crate::theme::Theme;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

mod agents;
pub(crate) use agents::HISTORY_WINDOW as AGENT_HISTORY_WINDOW;
mod collection;
use collection::{Collected, CollectionState, StartupMessage};
mod stats;
pub use stats::StatsScope;

/// Maximum data points kept for the live token-rate graph.
const GRAPH_HISTORY_LEN: usize = 200;
/// Window for the per-session and global token-rate summaries.
const TOKEN_RATE_WINDOW: Duration = Duration::from_secs(60);
/// Max concurrent summary jobs.
const MAX_SUMMARY_JOBS: usize = 3;
/// Max summary attempts per session before giving up.
const MAX_SUMMARY_RETRIES: u32 = 2;

/// Tokens observed during the last minute, without extrapolating startup data.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TokenRates {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenRates {
    pub(crate) fn total(&self) -> u64 {
        token_total(self.input, self.output, self.cache_write)
    }

    fn has_activity(&self) -> bool {
        self.input != 0 || self.output != 0 || self.cache_read != 0 || self.cache_write != 0
    }

    pub(crate) fn cache_reuse_percent(&self) -> Option<f64> {
        let input_side = self.input as f64 + self.cache_read as f64 + self.cache_write as f64;
        (input_side > 0.0).then(|| self.cache_read as f64 / input_side * 100.0)
    }

    fn add(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

struct TokenRateCounter {
    previous: u64,
    samples: VecDeque<(Instant, u64)>,
}

impl TokenRateCounter {
    fn new(total: u64) -> Self {
        Self {
            previous: total,
            samples: VecDeque::new(),
        }
    }

    fn update(&mut self, total: u64, now: Instant) -> u64 {
        if total < self.previous {
            // Transcript rotation or correction invalidates this category's history.
            self.samples.clear();
        }
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= TOKEN_RATE_WINDOW)
        {
            self.samples.pop_front();
        }
        let delta = total.saturating_sub(self.previous);
        if delta > 0 {
            self.samples.push_back((now, delta));
        }
        self.previous = total;
        delta
    }

    fn rate(&self, now: Instant) -> u64 {
        self.samples
            .iter()
            .filter(|(at, _)| now.duration_since(*at) < TOKEN_RATE_WINDOW)
            .map(|(_, delta)| delta)
            .sum()
    }
}

struct SessionTokenRates {
    input: TokenRateCounter,
    output: TokenRateCounter,
    cache_read: TokenRateCounter,
    cache_write: TokenRateCounter,
    models: HashMap<String, TokenRateCounter>,
    last_seen: Instant,
}

impl SessionTokenRates {
    fn new(session: &AgentSession, now: Instant) -> Self {
        Self {
            input: TokenRateCounter::new(session.total_input_tokens),
            output: TokenRateCounter::new(session.total_output_tokens),
            cache_read: TokenRateCounter::new(session.total_cache_read),
            cache_write: TokenRateCounter::new(session.total_cache_create),
            models: session
                .model_usage
                .iter()
                .map(|usage| {
                    (
                        usage.model.clone(),
                        TokenRateCounter::new(usage.total_tokens),
                    )
                })
                .collect(),
            last_seen: now,
        }
    }

    fn update(&mut self, session: &AgentSession, now: Instant) -> TokenRates {
        let previous_total = token_total(
            self.input.previous,
            self.output.previous,
            self.cache_write.previous,
        );
        // A reset or replacement of attribution invalidates model baselines,
        // including newly introduced Unknown entries. Do not count a replacement
        // baseline as fresh work just because its model identity changed.
        let reset_models = session.total_tokens() < previous_total
            || session.total_tokens() + session.total_cache_read
                < previous_total + self.cache_read.previous
            || session.total_cache_read < self.cache_read.previous
            || self.models.iter().any(|(model, counter)| {
                session
                    .model_usage
                    .iter()
                    .find(|usage| &usage.model == model)
                    .is_none_or(|usage| usage.total_tokens < counter.previous)
            });
        if reset_models {
            self.models.clear();
        }
        for usage in &session.model_usage {
            self.models
                .entry(usage.model.clone())
                .or_insert_with(|| {
                    // A model first used in an already observed session starts
                    // at zero so its first batch contributes to the rate.
                    TokenRateCounter::new(if reset_models { usage.total_tokens } else { 0 })
                })
                .update(usage.total_tokens, now);
        }
        self.last_seen = now;
        TokenRates {
            input: self.input.update(session.total_input_tokens, now),
            output: self.output.update(session.total_output_tokens, now),
            cache_read: self.cache_read.update(session.total_cache_read, now),
            cache_write: self.cache_write.update(session.total_cache_create, now),
        }
    }

    fn rates(&self, now: Instant) -> TokenRates {
        TokenRates {
            input: self.input.rate(now),
            output: self.output.rate(now),
            cache_read: self.cache_read.rate(now),
            cache_write: self.cache_write.rate(now),
        }
    }
}

/// Produce a terminal-safe fallback summary from a raw prompt.
fn sanitize_fallback(prompt: &str, max_len: usize) -> String {
    prompt
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .take(max_len)
        .collect()
}

/// Outcome of an Enter-key jump attempt. Distinct from `Option<String>` so
/// callers (notably `--exit-on-jump`) can tell a real terminal jump apart from
/// a no-op (unsupported terminal, or empty session list).
#[derive(Debug, PartialEq, Eq)]
pub enum JumpOutcome {
    /// Actually switched to a terminal pane/tab/window.
    Jumped,
    /// Tried to jump through an applicable backend, but the focus command failed.
    Failed(String),
    /// Unsupported terminal, or nothing selected — nothing happened.
    NoOp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NarrowTab {
    Work,
    Usage,
    System,
}

impl NarrowTab {
    pub const ALL: [Self; 3] = [Self::Work, Self::Usage, Self::System];

    pub fn label(self) -> &'static str {
        match self {
            Self::Work => "Work",
            Self::Usage => "Usage",
            Self::System => "System",
        }
    }

    pub fn shortcut(self) -> char {
        match self {
            Self::Work => 'w',
            Self::Usage => 'u',
            Self::System => 's',
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NarrowSection {
    Sessions,
    Projects,
    Context,
    TokenRate,
    Agents,
    Quota,
    Tokens,
    Models,
    Ports,
    Mcp,
}

impl NarrowSection {
    pub fn tab(self) -> NarrowTab {
        match self {
            Self::Sessions | Self::Projects | Self::Agents => NarrowTab::Work,
            Self::TokenRate | Self::Context | Self::Quota | Self::Tokens | Self::Models => {
                NarrowTab::Usage
            }
            Self::Ports | Self::Mcp => NarrowTab::System,
        }
    }
}

pub struct App {
    pub sessions: Vec<AgentSession>,
    pub selected: usize,
    /// Shared scope for the Tokens and Models panes.
    pub stats_scope: StatsScope,
    pub agent_scope: StatsScope,
    pub(crate) agent_history: agents::AgentHistory,
    pub(crate) save_agent_scope: fn(StatsScope) -> Result<(), String>,
    pub show_agents: bool,
    /// Combined rolling tokens/min, sampled for the sessions live at each tick.
    pub(crate) live_token_history: VecDeque<u64>,
    /// Injectable persistence boundary; tests never write the user's config.
    pub(crate) save_stats_scope: fn(StatsScope) -> Result<(), String>,
    pub should_quit: bool,
    /// Token rate per tick (delta). Ring buffer for the braille graph.
    pub token_rates: VecDeque<f64>,
    /// Cache-read deltas sampled alongside token_rates, including idle refreshes.
    pub(crate) cache_read_rates: VecDeque<f64>,
    /// Observed global activity, retained after sessions close until it expires.
    global_token_samples: VecDeque<(Instant, TokenRates)>,
    /// Account-level rate limits (Claude, Codex, etc.)
    pub rate_limits: Vec<RateLimitInfo>,
    /// Recent category deltas, keyed by (agent_cli, session_id).
    session_token_rates: HashMap<(String, String), SessionTokenRates>,
    collection: Option<CollectionState>,
    startup: Option<mpsc::Receiver<StartupMessage>>,
    /// Cached LLM-generated summaries, keyed by session_id.
    pub summaries: HashMap<String, String>,
    /// Session IDs currently being summarized.
    pending_summaries: HashSet<String>,
    /// Per-session retry count for failed summary attempts.
    summary_retries: HashMap<String, u32>,
    /// Channel to receive completed summaries from background threads.
    /// Tuple: (session_id, prompt, maybe_summary).
    summary_rx: mpsc::Receiver<(String, String, Option<String>)>,
    summary_tx: mpsc::Sender<(String, String, Option<String>)>,
    /// Ports left open by processes whose parent sessions have ended.
    pub orphan_ports: Vec<OrphanPort>,
    /// Transient status message shown in the footer (auto-clears after 3s).
    pub status_msg: Option<(String, Instant)>,
    /// Kill confirmation: (selected_index, timestamp). Expires after 2s.
    kill_confirm: Option<(usize, Instant)>,
    pub theme: Theme,
    pub show_context: bool,
    pub show_token_rate: bool,
    pub show_cache_read_rate: bool,
    pub(crate) save_cache_read_rate: fn(bool) -> Result<(), String>,
    /// Injectable persistence boundary; tests never write the user's config.
    pub(crate) save_panel_visibility: fn(&crate::config::PanelVisibility) -> Result<(), String>,
    pub show_quota: bool,
    pub show_tokens: bool,
    pub show_models: bool,
    pub show_projects: bool,
    pub show_ports: bool,
    pub show_sessions: bool,
    pub show_mcp: bool,
    pub narrow_tab: NarrowTab,
    pub active_narrow_section: Option<NarrowSection>,
    pub maximized_narrow_section: Option<NarrowSection>,
    /// MCP servers detected on the most recent tick (sourced from
    /// MultiCollector). Populated regardless of `show_mcp` so panel
    /// toggling doesn't cost a discovery roundtrip.
    pub mcp_servers: Vec<McpServer>,
    /// When true (default), mcp-server-owned rollouts are hidden from
    /// the sessions panel. Toggle with Shift+M.
    pub mcp_suppress_sessions: bool,
    pub config_open: bool,
    pub config_selected: usize,
    pub tree_view: bool,
    pub filter_text: String,
    pub filter_active: bool,
    pub show_timeline: bool,
    pub timeline_scroll: usize,
    pub show_file_audit: bool,
    /// Latest host metrics snapshot (None until first valid sample).
    pub host_metrics: Option<HostMetrics>,
    /// Aggregate metrics across all sessions (recomputed each tick).
    pub agent_aggregate: AgentAggregate,
    /// Help overlay (`?`) visibility.
    pub help_open: bool,
    /// View leader overlay (`v`) visibility.
    pub view_open: bool,
}

impl App {
    #[cfg(test)]
    pub fn new_with_config(
        theme: Theme,
        hidden_agents: &[String],
        panels: crate::config::PanelVisibility,
    ) -> Self {
        Self::new_with_config_and_claude_dirs(theme, hidden_agents, panels, &[])
    }

    pub fn new_with_config_and_claude_dirs(
        theme: Theme,
        hidden_agents: &[String],
        panels: crate::config::PanelVisibility,
        claude_config_dirs: &[PathBuf],
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let summaries = load_summary_cache();
        let collection = CollectionState::new(hidden_agents, claude_config_dirs);
        Self {
            sessions: Vec::new(),
            selected: 0,
            stats_scope: StatsScope::default(),
            agent_scope: StatsScope::AllLive,
            agent_history: Default::default(),
            save_agent_scope: crate::config::save_agent_scope,
            show_agents: panels.agents,
            live_token_history: VecDeque::with_capacity(GRAPH_HISTORY_LEN),
            save_stats_scope: crate::config::save_stats_scope,
            should_quit: false,
            token_rates: VecDeque::with_capacity(GRAPH_HISTORY_LEN),
            rate_limits: Vec::new(),
            cache_read_rates: VecDeque::with_capacity(GRAPH_HISTORY_LEN),
            global_token_samples: VecDeque::new(),
            session_token_rates: HashMap::new(),
            collection: Some(collection),
            startup: None,
            summaries,
            pending_summaries: HashSet::new(),
            summary_retries: HashMap::new(),
            summary_rx: rx,
            summary_tx: tx,
            orphan_ports: Vec::new(),
            status_msg: None,
            kill_confirm: None,
            theme,
            show_context: panels.context,
            show_token_rate: panels.token_rate,
            show_cache_read_rate: false,
            save_cache_read_rate: crate::config::save_cache_read_rate,
            save_panel_visibility: crate::config::save_panel_visibility,
            show_quota: panels.quota,
            show_tokens: panels.tokens,
            show_models: panels.models,
            show_projects: panels.projects,
            show_ports: panels.ports,
            show_sessions: panels.sessions,
            show_mcp: panels.mcp,
            narrow_tab: NarrowTab::Work,
            active_narrow_section: Some(NarrowSection::Sessions),
            maximized_narrow_section: None,
            mcp_servers: Vec::new(),
            mcp_suppress_sessions: true,
            config_open: false,
            config_selected: 0,
            tree_view: false,
            filter_text: String::new(),
            filter_active: false,
            show_timeline: false,
            timeline_scroll: 0,
            show_file_audit: false,
            host_metrics: None,
            agent_aggregate: AgentAggregate::default(),
            help_open: false,
            view_open: false,
        }
    }

    pub fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
        if self.help_open {
            self.view_open = false;
        }
    }

    pub fn toggle_view_menu(&mut self) {
        self.view_open = !self.view_open;
        if self.view_open {
            self.help_open = false;
        }
    }

    pub fn toggle_cache_read_rate(&mut self) {
        self.show_cache_read_rate = !self.show_cache_read_rate;
        if let Err(error) = (self.save_cache_read_rate)(self.show_cache_read_rate) {
            self.set_status(format!("cache chart save failed: {error}"));
        }
    }

    pub fn toggle_panel(&mut self, panel: u8) {
        match panel {
            0 => self.show_agents = !self.show_agents,
            1 => self.show_context = !self.show_context,
            2 => self.show_quota = !self.show_quota,
            3 => self.show_tokens = !self.show_tokens,
            4 => self.show_projects = !self.show_projects,
            5 => self.show_ports = !self.show_ports,
            6 => self.show_sessions = !self.show_sessions,
            7 => self.show_mcp = !self.show_mcp,
            8 => self.show_models = !self.show_models,
            9 => self.show_token_rate = !self.show_token_rate,
            _ => return,
        }
        self.persist_panel_visibility();
        self.clamp_narrow_tab();
    }

    /// Toggle whether mcp-server-owned rollouts are hidden from the
    /// sessions panel. Default is on; turning it off restores upstream
    /// behavior so the user can see exactly what mcp-server fd holding
    /// produces (mostly stale "Done" rows).
    pub fn toggle_mcp_session_suppression(&mut self) {
        self.mcp_suppress_sessions = !self.mcp_suppress_sessions;
        let label = if self.mcp_suppress_sessions {
            "on"
        } else {
            "off"
        };
        self.set_status(format!("mcp session suppression: {}", label));
    }

    fn persist_panel_visibility(&mut self) {
        let panels = crate::config::PanelVisibility {
            agents: self.show_agents,
            context: self.show_context,
            token_rate: self.show_token_rate,
            quota: self.show_quota,
            tokens: self.show_tokens,
            models: self.show_models,
            projects: self.show_projects,
            ports: self.show_ports,
            sessions: self.show_sessions,
            mcp: self.show_mcp,
        };
        if let Err(e) = (self.save_panel_visibility)(&panels) {
            self.set_status(format!("panels save failed: {}", e));
        }
    }

    pub fn toggle_file_audit(&mut self) {
        self.show_file_audit = !self.show_file_audit;
    }

    pub fn toggle_config(&mut self) {
        self.config_open = !self.config_open;
        if self.config_open {
            self.config_selected = 0;
        }
    }

    pub fn config_item_count(&self) -> usize {
        12 // theme + 10 panel toggles + cache chart
    }

    pub fn config_select_next(&mut self) {
        if self.config_selected + 1 < self.config_item_count() {
            self.config_selected += 1;
        }
    }

    pub fn config_select_prev(&mut self) {
        self.config_selected = self.config_selected.saturating_sub(1);
    }

    pub fn config_toggle_selected(&mut self) {
        match self.config_selected {
            0 => {
                self.cycle_theme();
                return;
            }
            1 => self.show_context = !self.show_context,
            2 => self.show_quota = !self.show_quota,
            3 => self.show_tokens = !self.show_tokens,
            4 => self.show_projects = !self.show_projects,
            5 => self.show_ports = !self.show_ports,
            6 => self.show_sessions = !self.show_sessions,
            7 => self.show_mcp = !self.show_mcp,
            8 => self.show_models = !self.show_models,
            9 => self.show_token_rate = !self.show_token_rate,
            10 => {
                self.toggle_cache_read_rate();
                return;
            }
            11 => self.show_agents = !self.show_agents,
            _ => return,
        }
        self.persist_panel_visibility();
        self.clamp_narrow_tab();
    }

    pub fn narrow_tab_visible(&self, tab: NarrowTab) -> bool {
        match tab {
            NarrowTab::Work => self.show_sessions || self.show_projects || self.show_agents,
            NarrowTab::Usage => {
                self.show_token_rate
                    || self.show_context
                    || self.show_quota
                    || self.show_tokens
                    || self.show_models
            }
            NarrowTab::System => self.show_ports || self.show_mcp,
        }
    }

    pub fn visible_narrow_tabs(&self) -> Vec<NarrowTab> {
        NarrowTab::ALL
            .into_iter()
            .filter(|&tab| self.narrow_tab_visible(tab))
            .collect()
    }

    pub fn active_narrow_tab(&self) -> Option<NarrowTab> {
        if self.narrow_tab_visible(self.narrow_tab) {
            Some(self.narrow_tab)
        } else {
            NarrowTab::ALL
                .into_iter()
                .find(|&tab| self.narrow_tab_visible(tab))
        }
    }

    pub fn set_narrow_tab(&mut self, tab: NarrowTab) {
        if self.narrow_tab_visible(tab) {
            self.narrow_tab = tab;
            self.clamp_narrow_section();
        }
    }

    pub fn select_next_narrow_tab(&mut self) {
        let tabs = self.visible_narrow_tabs();
        if tabs.is_empty() {
            return;
        }
        let current = self.active_narrow_tab().unwrap_or(tabs[0]);
        let pos = tabs.iter().position(|&tab| tab == current).unwrap_or(0);
        self.narrow_tab = tabs[(pos + 1) % tabs.len()];
        self.clamp_narrow_section();
    }

    pub fn select_prev_narrow_tab(&mut self) {
        let tabs = self.visible_narrow_tabs();
        if tabs.is_empty() {
            return;
        }
        let current = self.active_narrow_tab().unwrap_or(tabs[0]);
        let pos = tabs.iter().position(|&tab| tab == current).unwrap_or(0);
        self.narrow_tab = tabs[(pos + tabs.len() - 1) % tabs.len()];
        self.clamp_narrow_section();
    }

    fn clamp_narrow_tab(&mut self) {
        if let Some(tab) = self.active_narrow_tab() {
            self.narrow_tab = tab;
        }
        self.clamp_narrow_section();
    }

    pub fn narrow_section_visible(&self, section: NarrowSection) -> bool {
        match section {
            NarrowSection::Sessions => self.show_sessions,
            NarrowSection::Projects => self.show_projects,
            NarrowSection::Context => self.show_context,
            NarrowSection::TokenRate => self.show_token_rate,
            NarrowSection::Agents => self.show_agents,
            NarrowSection::Quota => self.show_quota,
            NarrowSection::Tokens => self.show_tokens,
            NarrowSection::Models => self.show_models,
            NarrowSection::Ports => self.show_ports,
            NarrowSection::Mcp => self.show_mcp,
        }
    }

    pub fn visible_narrow_sections(&self, tab: NarrowTab) -> Vec<NarrowSection> {
        let sections: &[NarrowSection] = match tab {
            NarrowTab::Work => &[
                NarrowSection::Agents,
                NarrowSection::Sessions,
                NarrowSection::Projects,
            ],
            NarrowTab::Usage => &[
                NarrowSection::TokenRate,
                NarrowSection::Context,
                NarrowSection::Quota,
                NarrowSection::Tokens,
                NarrowSection::Models,
            ],
            NarrowTab::System => &[NarrowSection::Ports, NarrowSection::Mcp],
        };
        sections
            .iter()
            .copied()
            .filter(|&section| self.narrow_section_visible(section))
            .collect()
    }

    pub fn active_narrow_section(&self) -> Option<NarrowSection> {
        let tab = self.active_narrow_tab()?;
        if let Some(section) = self.active_narrow_section {
            if section.tab() == tab && self.narrow_section_visible(section) {
                return Some(section);
            }
        }
        self.visible_narrow_sections(tab).into_iter().next()
    }

    pub fn set_active_narrow_section(&mut self, section: NarrowSection) {
        if self.narrow_section_visible(section) {
            self.narrow_tab = section.tab();
            self.active_narrow_section = Some(section);
            self.clamp_narrow_section();
        }
    }

    pub fn maximized_narrow_section(&self) -> Option<NarrowSection> {
        let section = self.maximized_narrow_section?;
        if self.active_narrow_tab() == Some(section.tab()) && self.narrow_section_visible(section) {
            Some(section)
        } else {
            None
        }
    }

    pub fn toggle_narrow_section_zoom(&mut self, section: NarrowSection) {
        if !self.narrow_section_visible(section) {
            return;
        }
        self.set_active_narrow_section(section);
        self.maximized_narrow_section = if self.maximized_narrow_section() == Some(section) {
            None
        } else {
            Some(section)
        };
    }

    pub fn maximize_active_narrow_section(&mut self) {
        if let Some(section) = self.active_narrow_section() {
            self.maximized_narrow_section = Some(section);
        }
    }

    pub fn restore_narrow_sections(&mut self) {
        self.maximized_narrow_section = None;
    }

    fn clamp_narrow_section(&mut self) {
        self.active_narrow_section = self.active_narrow_section();
        if self.maximized_narrow_section().is_none() {
            self.maximized_narrow_section = None;
        }
    }

    pub fn toggle_timeline(&mut self) {
        self.show_timeline = !self.show_timeline;
        self.timeline_scroll = 0;
    }

    pub fn cycle_theme(&mut self) {
        let names = crate::theme::THEME_NAMES;
        let current = names
            .iter()
            .position(|&n| n == self.theme.name)
            .unwrap_or(0);
        let next = (current + 1) % names.len();
        self.theme = Theme::by_name(names[next]).unwrap_or_default();
        if let Err(e) = crate::config::save_theme(names[next]) {
            self.set_status(format!("theme: {} (save failed: {})", names[next], e));
        } else {
            self.set_status(format!("theme: {}", names[next]));
        }
    }

    /// Set a transient status message that auto-clears after 3 seconds.
    pub fn set_status(&mut self, msg: String) {
        self.status_msg = Some((msg, Instant::now()));
    }

    /// Full refresh used by the TUI: collect monitored data, then generate and
    /// retry session summaries. Equivalent to [`App::tick_no_summaries`] followed
    /// by [`App::drain_and_retry_summaries`].
    pub fn tick(&mut self) {
        self.tick_no_summaries();
        if !self.initializing() {
            self.drain_and_retry_summaries();
        }
    }

    /// Refresh all monitored data WITHOUT spawning background summary jobs.
    ///
    /// `tick` additionally calls [`App::drain_and_retry_summaries`], which
    /// shells out to `claude --print` to generate session titles. Headless
    /// consumers (e.g. the web snapshot API) call this variant so they never
    /// spawn subprocesses or consume the user's Claude quota.
    pub fn tick_no_summaries(&mut self) {
        // Startup refresh requests coalesce into the next scheduled synchronous tick.
        let Some(collection) = self.collection.as_mut() else {
            return;
        };
        collection
            .collector
            .set_mcp_suppress(self.mcp_suppress_sessions);
        let result = collection.collect();
        self.apply_collected(result);
    }

    pub(crate) fn initializing(&self) -> bool {
        self.startup.is_some()
    }

    pub(crate) fn start_initial_collection(&mut self) -> std::io::Result<()> {
        if self.initializing() {
            return Ok(());
        }
        let mut state = self
            .collection
            .take()
            .expect("collection state owned by App");
        state.collector.set_mcp_suppress(self.mcp_suppress_sessions);
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("abtop-startup".into())
            .spawn(move || {
                let result = state.collect_progressively(|session| {
                    let _ = tx.send(StartupMessage::Session(Box::new(session.clone())));
                });
                let _ = tx.send(StartupMessage::Complete(Box::new((state, result))));
            })?;
        self.startup = Some(rx);
        Ok(())
    }

    /// Return true once the warmed collectors have returned to the UI thread.
    pub(crate) fn poll_initial_collection(&mut self) -> std::io::Result<bool> {
        let Some(rx) = &self.startup else {
            return Ok(false);
        };
        let mut updates = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(StartupMessage::Session(session)) => updates.push(*session),
                Ok(StartupMessage::Complete(result)) => {
                    // The final snapshot supersedes all queued provisional updates.
                    let (mut state, result) = *result;
                    state.collector.set_mcp_suppress(self.mcp_suppress_sessions);
                    self.collection = Some(state);
                    self.startup = None;
                    self.apply_collected(result);
                    return Ok(true);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(std::io::Error::other("initial collection worker failed"));
                }
            }
        }
        if !updates.is_empty() {
            let selected = self
                .sessions
                .get(self.selected)
                .map(|s| (s.agent_cli, s.session_id.clone()));
            let mut merged = std::mem::take(&mut self.sessions);
            let mut indices: HashMap<_, _> = merged
                .iter()
                .enumerate()
                .map(|(index, s)| ((s.agent_cli, s.session_id.clone()), index))
                .collect();
            for session in updates {
                let key = (session.agent_cli, session.session_id.clone());
                if let Some(&index) = indices.get(&key) {
                    merged[index] = session;
                } else {
                    indices.insert(key, merged.len());
                    merged.push(session);
                }
            }
            self.replace_sessions(merged, selected);
        }
        Ok(false)
    }

    fn replace_sessions(
        &mut self,
        mut sessions: Vec<AgentSession>,
        selected: Option<(&'static str, String)>,
    ) {
        sessions.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        self.selected = selected
            .and_then(|(provider, id)| {
                sessions
                    .iter()
                    .position(|s| s.agent_cli == provider && s.session_id == id)
            })
            .unwrap_or(self.selected)
            .min(sessions.len().saturating_sub(1));
        self.sessions = sessions;
        self.kill_confirm = None;
        self.clamp_selection_to_visible();
        self.agent_aggregate = AgentAggregate::from_sessions(&self.sessions);
    }

    fn apply_collected(&mut self, result: Collected) {
        let selected = self
            .sessions
            .get(self.selected)
            .map(|s| (s.agent_cli, s.session_id.clone()));
        self.replace_sessions(result.sessions, selected);
        self.orphan_ports = result.orphan_ports;
        self.mcp_servers = result.mcp_servers;
        self.host_metrics = result.host_metrics;
        self.rate_limits = result.rate_limits;
        let now = Instant::now();
        self.update_session_token_rates(now);
        promote_waiting_to_rate_limited(&mut self.sessions, &self.rate_limits);
        self.record_agents(now);
    }

    pub(crate) fn update_session_token_rates(&mut self, now: Instant) {
        // Keep baselines through brief discovery gaps, but do not accumulate
        // stale session state or attribute a long absence's tokens to one minute.
        self.session_token_rates
            .retain(|_, rates| now.duration_since(rates.last_seen) < TOKEN_RATE_WINDOW);
        let mut seen = HashSet::new();
        let mut delta = TokenRates::default();
        for session in &self.sessions {
            let key = (session.agent_cli.to_string(), session.session_id.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            delta.add(
                &self
                    .session_token_rates
                    .entry(key)
                    .or_insert_with(|| SessionTokenRates::new(session, now))
                    .update(session, now),
            );
        }
        self.record_global_token_delta(now, delta);
        self.live_token_history
            .push_back(self.live_token_rates(now).total());
        if self.live_token_history.len() > GRAPH_HISTORY_LEN {
            self.live_token_history.pop_front();
        }
    }

    pub(crate) fn record_global_token_delta(&mut self, now: Instant, delta: TokenRates) {
        self.token_rates.push_back(delta.total() as f64);
        self.cache_read_rates.push_back(delta.cache_read as f64);
        if self.token_rates.len() > GRAPH_HISTORY_LEN {
            self.token_rates.pop_front();
        }
        if self.cache_read_rates.len() > GRAPH_HISTORY_LEN {
            self.cache_read_rates.pop_front();
        }
        while self
            .global_token_samples
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= TOKEN_RATE_WINDOW)
        {
            self.global_token_samples.pop_front();
        }
        if delta.has_activity() {
            self.global_token_samples.push_back((now, delta));
        }
    }

    pub(crate) fn global_token_rates(&self, now: Instant) -> TokenRates {
        let mut rates = TokenRates::default();
        for (_, delta) in self
            .global_token_samples
            .iter()
            .filter(|(at, _)| now.duration_since(*at) < TOKEN_RATE_WINDOW)
        {
            rates.add(delta);
        }
        rates
    }

    pub(crate) fn clear_global_token_history(&mut self) {
        self.token_rates.clear();
        self.cache_read_rates.clear();
        self.global_token_samples.clear();
    }

    pub(crate) fn session_token_rates(&self, session: &AgentSession, now: Instant) -> TokenRates {
        self.session_token_rates
            .get(&(session.agent_cli.to_string(), session.session_id.clone()))
            .map(|rates| rates.rates(now))
            .unwrap_or_default()
    }

    pub(crate) fn model_token_rate(
        &self,
        session: &AgentSession,
        model: &str,
        now: Instant,
    ) -> u64 {
        self.session_token_rates
            .get(&(session.agent_cli.to_string(), session.session_id.clone()))
            .and_then(|rates| rates.models.get(model))
            .map(|counter| counter.rate(now))
            .unwrap_or_default()
    }

    /// Drain completed summary results and spawn retries. Does NOT recollect
    /// sessions, so it is safe for `--once` mode (stable snapshot).
    pub fn drain_and_retry_summaries(&mut self) {
        while let Ok((sid, prompt, maybe_summary)) = self.summary_rx.try_recv() {
            self.pending_summaries.remove(&sid);
            match maybe_summary {
                Some(summary) => {
                    self.summary_retries.remove(&sid);
                    self.summaries.insert(sid, summary);
                    save_summary_cache(&self.summaries);
                }
                None => {
                    let count = self.summary_retries.entry(sid.clone()).or_insert(0);
                    *count += 1;
                    if *count >= MAX_SUMMARY_RETRIES {
                        // Exhausted — store sanitized fallback using prompt from worker
                        self.summaries.insert(sid, sanitize_fallback(&prompt, 80));
                        save_summary_cache(&self.summaries);
                    }
                }
            }
        }

        // Spawn summary jobs for sessions that need one
        for s in &self.sessions {
            let retries = self
                .summary_retries
                .get(&s.session_id)
                .copied()
                .unwrap_or(0);
            let has_input = !s.initial_prompt.is_empty() || !s.first_assistant_text.is_empty();
            if has_input
                && !self.summaries.contains_key(&s.session_id)
                && !self.pending_summaries.contains(&s.session_id)
                && self.pending_summaries.len() < MAX_SUMMARY_JOBS
                && retries < MAX_SUMMARY_RETRIES
            {
                self.pending_summaries.insert(s.session_id.clone());
                let sid = s.session_id.clone();
                let prompt = s.initial_prompt.clone();
                let assistant_text = s.first_assistant_text.clone();
                let tx = self.summary_tx.clone();
                std::thread::spawn(move || {
                    let result = generate_summary(&prompt, &assistant_text);
                    let fallback_text = if prompt.is_empty() {
                        assistant_text
                    } else {
                        prompt
                    };
                    let _ = tx.send((sid, fallback_text, result));
                });
            }
        }
    }

    pub fn has_pending_summaries(&self) -> bool {
        !self.pending_summaries.is_empty()
    }

    /// True if any session still qualifies for a summary retry.
    pub fn has_retryable_summaries(&self) -> bool {
        self.sessions.iter().any(|s| {
            (!s.initial_prompt.is_empty() || !s.first_assistant_text.is_empty())
                && !self.summaries.contains_key(&s.session_id)
                && !self.pending_summaries.contains(&s.session_id)
                && self
                    .summary_retries
                    .get(&s.session_id)
                    .copied()
                    .unwrap_or(0)
                    < MAX_SUMMARY_RETRIES
        })
    }

    /// Returns indices of sessions matching the current filter.
    pub fn visible_indices(&self) -> Vec<usize> {
        if self.filter_text.is_empty() {
            return (0..self.sessions.len()).collect();
        }
        let query = self.filter_text.to_lowercase();
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| Self::session_matches(s, &query))
            .map(|(i, _)| i)
            .collect()
    }

    fn session_matches(s: &AgentSession, query: &str) -> bool {
        s.project_name.to_lowercase().contains(query)
            || s.model.to_lowercase().contains(query)
            || s.session_id.to_lowercase().contains(query)
            || s.initial_prompt.to_lowercase().contains(query)
            || s.cwd.to_lowercase().contains(query)
            || format!("{:?}", s.status).to_lowercase().contains(query)
    }

    /// Ensure `selected` points to a session included in the current filter.
    /// No-op when no sessions match; otherwise snaps to the first visible.
    fn clamp_selection_to_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if !visible.contains(&self.selected) {
            self.selected = visible[0];
        }
    }

    pub fn filter_push(&mut self, c: char) {
        self.filter_text.push(c);
        self.clamp_selection_to_visible();
    }

    pub fn filter_pop(&mut self) {
        self.filter_text.pop();
        self.clamp_selection_to_visible();
    }

    pub fn clear_filter(&mut self) {
        self.filter_active = false;
        self.filter_text.clear();
    }

    pub fn select_next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            if pos + 1 < visible.len() {
                self.selected = visible[pos + 1];
            }
        } else {
            self.selected = visible[0];
        }
    }

    pub fn select_prev(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            if pos > 0 {
                self.selected = visible[pos - 1];
            }
        } else {
            self.selected = *visible.last().unwrap();
        }
    }

    pub fn select_session(&mut self, index: usize) {
        if index < self.sessions.len() && self.visible_indices().contains(&index) {
            self.selected = index;
        }
    }

    pub fn kill_selected(&mut self) {
        if self.initializing() {
            return;
        }
        if self.sessions.is_empty() {
            return;
        }
        let session = &self.sessions[self.selected];
        if matches!(session.status, SessionStatus::Done | SessionStatus::Unknown) {
            return;
        }

        // Check if we have a pending confirmation for this exact session
        if let Some((idx, ts)) = self.kill_confirm.take() {
            if idx == self.selected && ts.elapsed().as_secs() < 2 {
                // Confirmed — verify PID still runs a killable agent before killing
                let pid = session.pid;
                let verified = std::process::Command::new("ps")
                    .args(["-p", &pid.to_string(), "-o", "command="])
                    .output()
                    .ok()
                    .map(|output| {
                        let cmd = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        is_killable_agent_command(&cmd)
                    })
                    .unwrap_or(false);
                if !verified {
                    self.set_status(format!("PID {} is no longer a known agent process", pid));
                    return;
                }
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .output();
                self.tick();
                return;
            }
        }

        // First press — ask for confirmation
        let name = self
            .summaries
            .get(&session.session_id)
            .cloned()
            .unwrap_or_else(|| format!("PID {}", session.pid));
        self.kill_confirm = Some((self.selected, Instant::now()));
        self.set_status(format!("Press x again to kill: {}", name));
    }

    /// Kill all orphan port processes (Shift+X).
    /// Does a fresh port scan and validates PID identity + port ownership
    /// immediately before sending any signals to avoid PID reuse / stale cache issues.
    pub fn kill_orphan_ports(&mut self) {
        if self.initializing() {
            return;
        }
        use crate::collector::process::get_listening_ports;

        // Fresh port scan right now — don't rely on cached data
        let fresh_ports = get_listening_ports();

        for orphan in &self.orphan_ports {
            // 1. Verify PID still listens on the expected port
            let still_listening = fresh_ports
                .get(&orphan.pid)
                .is_some_and(|ports| ports.contains(&orphan.port));
            if !still_listening {
                continue;
            }
            // 2. Verify PID still runs the expected command (full match, not substring)
            if let Ok(output) = std::process::Command::new("ps")
                .args(["-p", &orphan.pid.to_string(), "-o", "command="])
                .output()
            {
                let current_cmd = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if current_cmd == orphan.command {
                    let _ = std::process::Command::new("kill")
                        .args([&orphan.pid.to_string()])
                        .output();
                }
            }
        }
        // Re-collect to reflect changes
        self.tick();
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Jump to the terminal running the selected session's agent process.
    /// Delegates to the terminal-jumper registry (cmux / tmux / iTerm2);
    /// see [`crate::jump`]. No-op when nothing is selected or no backend
    /// recognizes the process.
    pub fn jump_to_session(&mut self) -> JumpOutcome {
        if self.sessions.is_empty() {
            return JumpOutcome::NoOp;
        }
        let target_pid = self.sessions[self.selected].pid;
        crate::jump::run_jump(target_pid)
    }

    /// Get the display summary for a session: LLM summary > "..." if pending > raw prompt > "—"
    /// Done sessions skip pending state to avoid stuck "..." display.
    pub fn session_summary(&self, session: &AgentSession) -> String {
        if let Some(summary) = self.summaries.get(&session.session_id) {
            summary.clone()
        } else if matches!(session.status, SessionStatus::Done) {
            // Done sessions: don't wait for pending summary, show fallback immediately
            if !session.initial_prompt.is_empty() {
                sanitize_fallback(&session.initial_prompt, 80)
            } else if !session.first_assistant_text.is_empty() {
                sanitize_fallback(&session.first_assistant_text, 80)
            } else {
                "—".to_string()
            }
        } else if self.pending_summaries.contains(&session.session_id) {
            // Animate dots: . → .. → ... (cycles every ~1.5s at 2s tick)
            let dots = match (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 500)
                % 3
            {
                0 => ".",
                1 => "..",
                _ => "...",
            };
            dots.to_string()
        } else if !session.initial_prompt.is_empty() {
            sanitize_fallback(&session.initial_prompt, 80)
        } else if !session.first_assistant_text.is_empty() {
            sanitize_fallback(&session.first_assistant_text, 80)
        } else {
            "—".to_string()
        }
    }
}

/// Call `claude --print` via stdin pipe to summarize a prompt.
/// Returns `None` on timeout so the caller can retry later.
fn generate_summary(prompt: &str, assistant_text: &str) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    // Build input from user prompt and/or first assistant response
    let user_part: String = prompt.chars().take(200).collect();
    let assistant_part: String = assistant_text.chars().take(200).collect();

    let context = if !user_part.is_empty() && !assistant_part.is_empty() {
        format!(
            "User message: {}\n\nAssistant response: {}",
            user_part, assistant_part
        )
    } else if !assistant_part.is_empty() {
        format!("Assistant response: {}", assistant_part)
    } else {
        format!("User message: {}", user_part)
    };

    let request = format!(
        "You are a conversation title generator. Given the conversation below, create a short title (3-5 words) that describes the session's main topic. Be specific and actionable. Do NOT output generic titles like 'New conversation' or 'Initial setup'. Output ONLY the title, no quotes, no explanation.\n\n{}",
        context
    );

    let mut child = match Command::new("claude")
        .args(["--print", "-"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Some(sanitize_fallback(prompt, 80)),
    };

    // Write prompt via stdin (no shell injection)
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(request.as_bytes());
    }

    // Run wait_with_output in a helper thread so we can apply a bounded timeout.
    // This drains stdout internally, avoiding pipe-full deadlock.
    let child_pid = child.id();
    let (wo_tx, wo_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = wo_tx.send(child.wait_with_output());
    });

    let result = match wo_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(r) => r,
        Err(_) => {
            // Timeout or disconnected — kill the child so the helper thread can exit.
            let _ = std::process::Command::new("kill")
                .args(["-9", &child_pid.to_string()])
                .status();
            return None;
        }
    };

    let fallback = sanitize_fallback(prompt, 80);

    match result {
        Ok(output) if output.status.success() => {
            let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let lower = raw.to_lowercase();
            // Reject empty, too long, generic, or prompt-echo outputs
            if raw.is_empty()
                || raw.chars().count() > 80
                || raw.contains("Summarize")
                || raw.starts_with("- ")
                || lower.contains("new conversation")
                || lower.contains("initial setup")
                || lower.contains("initial project")
                || lower.contains("initial conversation")
                || lower.starts_with("greeting")
            {
                Some(fallback)
            } else {
                Some(raw.trim_matches('"').trim_matches('\'').to_string())
            }
        }
        _ => Some(fallback),
    }
}

/// Cache directory: ~/.cache/abtop/
fn cache_dir() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"))
        .join("abtop")
}

fn cache_path() -> std::path::PathBuf {
    cache_dir().join("summaries.json")
}

fn load_summary_cache() -> HashMap<String, String> {
    let path = cache_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let mut cache: HashMap<String, String> =
                serde_json::from_str(&content).unwrap_or_default();
            // Purge polluted or old truncated-fallback entries so they regenerate
            let before = cache.len();
            cache.retain(|_, v| !v.contains("You are a conversation tit") && !v.ends_with('…'));
            if cache.len() < before {
                // Persist cleaned cache
                let _ = std::fs::create_dir_all(cache_dir());
                let _ = std::fs::write(&path, serde_json::to_string(&cache).unwrap_or_default());
            }
            cache
        }
        Err(_) => HashMap::new(),
    }
}

fn save_summary_cache(summaries: &HashMap<String, String>) {
    let path = cache_path();
    let _ = std::fs::create_dir_all(cache_dir());
    if let Ok(json) = serde_json::to_string(summaries) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// Threshold above which a rate-limited bucket is surfaced as RateLimited
/// in the session list. 90% leaves enough headroom to catch near-saturation
/// before the account actually blocks.
const RATE_LIMITED_PCT: f64 = 90.0;

/// Promote Waiting sessions to RateLimited when a rate limit from the SAME
/// agent CLI is over `RATE_LIMITED_PCT`. Matching on source avoids a
/// Claude-only saturation freezing Codex sessions and vice versa.
fn promote_waiting_to_rate_limited(sessions: &mut [AgentSession], rate_limits: &[RateLimitInfo]) {
    if rate_limits.is_empty() {
        return;
    }
    for s in sessions.iter_mut() {
        if s.status != SessionStatus::Waiting {
            continue;
        }
        let over = rate_limits.iter().any(|rl| {
            rl.source == s.agent_cli
                && (rl.five_hour_pct.unwrap_or(0.0) > RATE_LIMITED_PCT
                    || rl.seven_day_pct.unwrap_or(0.0) > RATE_LIMITED_PCT)
        });
        if over {
            s.status = SessionStatus::RateLimited;
        }
    }
}

fn is_supported_agent_command(cmd: &str) -> bool {
    crate::collector::process::cmd_has_binary(cmd, "claude")
        || crate::collector::process::cmd_has_binary(cmd, "codex")
        || crate::collector::process::cmd_has_binary(cmd, "opencode")
        || crate::collector::pi::cmd_matches_harness(cmd, "omp")
        || crate::collector::pi::cmd_matches_harness(cmd, "pi")
}

fn is_killable_agent_command(cmd: &str) -> bool {
    is_supported_agent_command(cmd)
        && !(crate::collector::process::cmd_has_binary(cmd, "codex") && cmd.contains(" app-server"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LaunchSurface, ModelUsage};

    fn add_model_usage(session: &mut AgentSession, model: &str, tokens: [u64; 4]) {
        let [input, output, cache_read, cache_write] = tokens;
        session.total_input_tokens += input;
        session.total_output_tokens += output;
        session.total_cache_read += cache_read;
        session.total_cache_create += cache_write;
        ModelUsage::add(
            &mut session.model_usage,
            model,
            token_total(input, output, cache_write),
        );
    }

    #[test]
    fn model_rates_count_new_work_and_expire_without_extrapolating() {
        let start = Instant::now();
        let mut session = waiting_session("claude");
        add_model_usage(&mut session, "model-a", [100, 50, 10_000, 25]);
        let mut rates = SessionTokenRates::new(&session, start);
        assert_eq!(rates.models["model-a"].rate(start), 0);

        add_model_usage(&mut session, "model-a", [10, 20, 3_000, 40]);
        add_model_usage(&mut session, "model-b", [1, 2, 400, 4]);
        add_model_usage(&mut session, "", [0, 0, 400, 0]);
        let now = start + Duration::from_secs(2);
        rates.update(&session, now);
        assert_eq!(rates.models["model-a"].rate(now), 70);
        assert_eq!(rates.models["model-b"].rate(now), 7);
        assert!(!rates.models.contains_key("Unknown"));
        let categories = rates.rates(now);
        assert_eq!(
            rates
                .models
                .values()
                .map(|counter| counter.rate(now))
                .sum::<u64>(),
            categories.input + categories.output + categories.cache_write
        );

        // Neither unchanged polls nor cache reads add to model rates.
        rates.update(&session, start + Duration::from_secs(3));
        add_model_usage(&mut session, "model-a", [0, 0, 1_000, 0]);
        rates.update(&session, start + Duration::from_secs(4));
        assert_eq!(
            rates.models["model-a"].rate(start + Duration::from_secs(61)),
            70
        );
        for counter in rates.models.values() {
            assert_eq!(counter.rate(start + Duration::from_secs(62)), 0);
        }
    }

    #[test]
    fn model_rates_rebaseline_replacements_and_decreasing_counters() {
        let start = Instant::now();
        let mut session = waiting_session("codex");
        add_model_usage(&mut session, "model-a", [100, 0, 0, 0]);
        let mut rates = SessionTokenRates::new(&session, start);
        add_model_usage(&mut session, "model-a", [20, 0, 0, 0]);
        rates.update(&session, start + Duration::from_secs(2));

        // A new identity replacing the old attribution is a baseline even if
        // the overall total grew. It must not appear as a new-model spike.
        session = waiting_session("codex");
        add_model_usage(&mut session, "Unknown", [500, 0, 0, 0]);
        let now = start + Duration::from_secs(4);
        rates.update(&session, now);
        assert!(!rates.models.contains_key("model-a"));
        assert_eq!(rates.models["Unknown"].rate(now), 0);
        add_model_usage(&mut session, "model-a", [10, 0, 0, 0]);
        let now = start + Duration::from_secs(6);
        rates.update(&session, now);
        assert_eq!(rates.models["model-a"].rate(now), 10);

        session = waiting_session("codex");
        add_model_usage(&mut session, "model-a", [5, 0, 0, 0]);
        let now = start + Duration::from_secs(8);
        rates.update(&session, now);
        assert_eq!(rates.models["model-a"].rate(now), 0);
        add_model_usage(&mut session, "model-a", [3, 0, 0, 0]);
        let now = start + Duration::from_secs(10);
        rates.update(&session, now);
        assert_eq!(rates.models["model-a"].rate(now), 3);

        // Reclassifying cached tokens as input is a correction, even with the
        // same model and unchanged overall total; it is not new active work.
        add_model_usage(&mut session, "model-a", [0, 0, 100, 0]);
        rates.update(&session, start + Duration::from_secs(12));
        session.total_cache_read -= 10;
        session.total_input_tokens += 10;
        session.model_usage[0].total_tokens += 10;
        let now = start + Duration::from_secs(14);
        rates.update(&session, now);
        assert_eq!(rates.models["model-a"].rate(now), 0);
    }

    fn waiting_session(cli: &'static str) -> AgentSession {
        AgentSession {
            agent_cli: cli,
            launch_surface: LaunchSurface::Cli,
            pid: 1,
            session_id: String::new(),
            cwd: String::new(),
            project_name: String::new(),
            started_at: 0,
            status: SessionStatus::Waiting,
            model: String::new(),
            effort: String::new(),
            context_percent: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read: 0,
            total_cache_create: 0,
            model_usage: Vec::new(),
            turn_count: 0,
            compaction_count: 0,
            current_tasks: vec![],
            version: String::new(),
            git_branch: String::new(),
            mem_mb: 0,
            token_history: vec![],
            context_history: vec![],
            context_window: 0,
            subagent_counts: Default::default(),
            subagents: vec![],
            pending_workflows: 0,
            pending_bg_agents: 0,
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![],
            initial_prompt: String::new(),
            first_assistant_text: String::new(),
            chat_messages: vec![],
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            file_accesses: vec![],
            config_root: String::new(),
            git_added: 0,
            git_modified: 0,
        }
    }

    struct ControlledCollector {
        session: AgentSession,
        gate: Option<mpsc::Receiver<()>>,
        entered: mpsc::Sender<bool>,
        calls: u64,
    }

    impl crate::collector::AgentCollector for ControlledCollector {
        fn collect(&mut self, shared: &crate::collector::SharedProcessData) -> Vec<AgentSession> {
            self.entered.send(shared.mcp_suppress).unwrap();
            if let Some(gate) = self.gate.take() {
                gate.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            self.calls += 1;
            self.session.total_input_tokens = 100 + self.calls;
            vec![self.session.clone()]
        }
    }

    impl crate::collector::StartupCollector for ControlledCollector {}

    fn controlled_app() -> (App, Vec<mpsc::Sender<()>>, Vec<mpsc::Receiver<bool>>) {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        let mut collectors: Vec<Box<dyn crate::collector::StartupCollector + Send>> = Vec::new();
        let mut gates = Vec::new();
        let mut entered = Vec::new();
        for (index, cli) in ["claude", "codex", "opencode"].into_iter().enumerate() {
            let (tx, rx) = mpsc::channel();
            let (entered_tx, entered_rx) = mpsc::channel();
            let mut session = waiting_session(cli);
            session.session_id = cli.into();
            session.started_at = index as u64;
            session.project_name = cli.into();
            session.cwd = "/nonexistent-abtop-startup-test".into();
            session.status = SessionStatus::Thinking;
            collectors.push(Box::new(ControlledCollector {
                session,
                gate: Some(rx),
                entered: entered_tx,
                calls: 0,
            }));
            gates.push(tx);
            entered.push(entered_rx);
        }
        app.collection.as_mut().unwrap().collector =
            crate::collector::MultiCollector::controlled(collectors);
        (app, gates, entered)
    }

    #[test]
    fn startup_renders_progressively_and_returns_warmed_collection_once() {
        let (mut app, gates, entered) = controlled_app();
        app.start_initial_collection().unwrap();
        assert!(entered[0].recv_timeout(Duration::from_secs(5)).unwrap());
        // The same renderer used by the TUI can draw while provider one is blocked.
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &app)).unwrap();
        assert!(app.sessions.is_empty());
        assert!(app.initializing());
        assert!(app.token_rates.is_empty());
        assert!(app.agent_bars(1, Instant::now())[0].is_none());
        assert!(app.pending_summaries.is_empty());
        gates[0].send(()).unwrap();
        assert!(entered[1].recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(!app.poll_initial_collection().unwrap());
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.sessions[app.selected].agent_cli, "claude");
        app.kill_selected();
        app.kill_orphan_ports();
        assert!(app.kill_confirm.is_none());
        // Simulate a stale pending confirmation; every new row set clears it.
        app.kill_confirm = Some((0, Instant::now()));
        app.tick();
        app.tick_no_summaries();
        app.tick();
        assert!(app.token_rates.is_empty());
        assert!(app.pending_summaries.is_empty());
        app.toggle_mcp_session_suppression();
        gates[1].send(()).unwrap();
        assert!(entered[2].recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(!app.poll_initial_collection().unwrap());
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.sessions[app.selected].agent_cli, "claude");
        assert_eq!(app.selected, 1);
        assert!(app.kill_confirm.is_none());
        app.select_session(0);
        gates[2].send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.poll_initial_collection().unwrap() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!app.initializing());
        assert_eq!(app.sessions.len(), 3);
        assert_eq!(app.sessions[app.selected].agent_cli, "codex");
        assert!(!app.collection.as_ref().unwrap().collector.mcp_suppress);
        assert_eq!(app.token_rates.len(), 1);
        assert_eq!(app.cache_read_rates.len(), 1);
        assert_eq!(app.live_token_history.len(), 1);
        assert_eq!(app.global_token_rates(Instant::now()).total(), 0);
        assert_eq!(app.session_token_rates.len(), 3);
        assert!(app.agent_bars(1, Instant::now())[0].is_some());
        assert!(!app.poll_initial_collection().unwrap());
        assert_eq!(app.token_rates.len(), 1);
        // Existing collectors, rather than new instances, handle the next sync tick.
        app.tick_no_summaries();
        for receiver in entered {
            assert!(!receiver.recv_timeout(Duration::from_secs(5)).unwrap());
        }
        assert!(app.sessions.iter().all(|s| s.total_input_tokens == 102));
        assert_eq!(app.global_token_rates(Instant::now()).input, 3);
        assert_eq!(app.token_rates.len(), 2);
    }

    #[test]
    fn quitting_drops_startup_receiver_without_waiting_for_collectors() {
        let (mut app, gates, entered) = controlled_app();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            app.start_initial_collection().unwrap();
            entered[0].recv_timeout(Duration::from_secs(5)).unwrap();
            app.quit();
            assert!(app.should_quit);
            drop(app);
            dropped_tx.send(()).unwrap();
            // Keep the test's provider notification receivers alive until released.
            for receiver in entered.into_iter().skip(1) {
                receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        });
        dropped_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // No provider can have finished before the App was dropped.
        for gate in gates {
            gate.send(()).unwrap();
        }
        worker.join().unwrap();
    }

    #[test]
    fn startup_worker_failure_is_reported_as_io_error() {
        struct FailingCollector;
        impl crate::collector::AgentCollector for FailingCollector {
            fn collect(&mut self, _: &crate::collector::SharedProcessData) -> Vec<AgentSession> {
                panic!("controlled startup failure");
            }
        }
        impl crate::collector::StartupCollector for FailingCollector {}
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        app.collection.as_mut().unwrap().collector =
            crate::collector::MultiCollector::controlled(vec![Box::new(FailingCollector)]);
        app.start_initial_collection().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match app.poll_initial_collection() {
                Err(error) => {
                    assert_eq!(error.to_string(), "initial collection worker failed");
                    break;
                }
                Ok(_) => {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    struct SessionBySessionCollector {
        sessions: Vec<AgentSession>,
        first_published: mpsc::Sender<()>,
        continue_scan: Option<mpsc::Receiver<()>>,
    }

    impl crate::collector::AgentCollector for SessionBySessionCollector {
        fn collect(&mut self, _: &crate::collector::SharedProcessData) -> Vec<AgentSession> {
            assert!(
                self.continue_scan.is_none(),
                "startup must use its completion observer"
            );
            self.sessions.clone()
        }
    }

    impl crate::collector::StartupCollector for SessionBySessionCollector {
        fn collect_startup(
            &mut self,
            _: &crate::collector::SharedProcessData,
            completed: &mut dyn FnMut(&mut AgentSession),
        ) -> Vec<AgentSession> {
            for (index, session) in self.sessions.iter_mut().enumerate() {
                completed(session);
                if index == 0 {
                    self.first_published.send(()).unwrap();
                    self.continue_scan
                        .take()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                }
            }
            self.sessions.clone()
        }
    }

    #[test]
    fn startup_displays_completed_session_before_same_provider_finishes() {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        let mut first = waiting_session("claude");
        first.session_id = "first".into();
        first.project_name = "early-project".into();
        first.cwd = "/nonexistent-abtop-startup-test".into();
        first.status = SessionStatus::Thinking;
        first.total_input_tokens = 120;
        crate::model::ModelUsage::add(&mut first.model_usage, "test-model", 120);
        let mut second = first.clone();
        second.session_id = "second".into();
        second.project_name = "later-project".into();
        second.started_at = 1;
        let (published_tx, published_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        app.collection.as_mut().unwrap().collector = crate::collector::MultiCollector::controlled(
            vec![Box::new(SessionBySessionCollector {
                sessions: vec![first, second],
                first_published: published_tx,
                continue_scan: Some(continue_rx),
            })],
        );
        app.start_initial_collection().unwrap();
        published_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!app.poll_initial_collection().unwrap());
        assert!(app.initializing());
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.usage_stats(Instant::now()).total_tokens(), 120);
        assert_eq!(app.global_agent_counts().claude.working, 1);
        assert!(app.token_rates.is_empty());
        assert!(app.agent_bars(1, Instant::now())[0].is_none());
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, &app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("early-project"));
        assert!(rendered.contains("test-model"));
        assert!(!rendered.contains("later-project"));
        continue_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.poll_initial_collection().unwrap() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.sessions[app.selected].session_id, "first");
        app.stats_scope = StatsScope::AllLive;
        assert_eq!(app.usage_stats(Instant::now()).total_tokens(), 240);
        assert_eq!(app.token_rates.len(), 1);
        assert_eq!(app.global_token_rates(Instant::now()).total(), 0);
        app.tick_no_summaries();
        assert_eq!(app.sessions.len(), 2);
        assert_eq!(app.token_rates.len(), 2);
        assert_eq!(app.global_token_rates(Instant::now()).total(), 0);
    }

    #[test]
    fn startup_upserts_sessions_and_final_snapshot_removes_provisional_rows() {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        let state = app.collection.take().unwrap();
        let (tx, rx) = mpsc::channel();
        app.startup = Some(rx);
        let mut session = waiting_session("claude");
        session.session_id = "shared-id".into();
        session.project_name = "keep".into();
        session.total_input_tokens = 10;
        tx.send(StartupMessage::Session(Box::new(session.clone())))
            .unwrap();
        session.total_input_tokens = 20;
        tx.send(StartupMessage::Session(Box::new(session.clone())))
            .unwrap();
        assert!(!app.poll_initial_collection().unwrap());
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.usage_stats(Instant::now()).total_tokens(), 20);
        app.kill_confirm = Some((0, Instant::now()));
        app.filter_text = "keep".into();
        let mut other_provider = session.clone();
        other_provider.agent_cli = "codex";
        other_provider.project_name = "filtered".into();
        other_provider.started_at = 2;
        tx.send(StartupMessage::Session(Box::new(other_provider)))
            .unwrap();
        session.total_input_tokens = 30;
        tx.send(StartupMessage::Session(Box::new(session.clone())))
            .unwrap();
        assert!(!app.poll_initial_collection().unwrap());
        assert_eq!(
            app.sessions.len(),
            2,
            "provider is part of session identity"
        );
        assert_eq!(app.visible_indices(), vec![1]);
        assert_eq!(app.selected, 1);
        assert!(app.kill_confirm.is_none());
        assert_eq!(app.usage_stats(Instant::now()).total_tokens(), 30);
        assert!(app.token_rates.is_empty());
        // A queued session update and a previously displayed row both yield to final data.
        let mut removed = session.clone();
        removed.session_id = "removed".into();
        tx.send(StartupMessage::Session(Box::new(removed))).unwrap();
        tx.send(StartupMessage::Complete(Box::new((
            state,
            Collected {
                sessions: vec![session],
                orphan_ports: vec![],
                mcp_servers: vec![],
                host_metrics: None,
                rate_limits: vec![],
            },
        ))))
        .unwrap();
        assert!(app.poll_initial_collection().unwrap());
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.sessions[app.selected].session_id, "shared-id");
        assert_eq!(app.usage_stats(Instant::now()).total_tokens(), 30);
        assert_eq!(app.token_rates.len(), 1);
        assert_eq!(app.global_token_rates(Instant::now()).total(), 0);
    }

    #[test]
    fn default_startup_waits_for_batch_finalization_and_enriches_before_publication() {
        struct BatchCollector {
            started: mpsc::Sender<()>,
            finish: mpsc::Receiver<()>,
            sessions: Vec<AgentSession>,
        }
        impl crate::collector::AgentCollector for BatchCollector {
            fn collect(&mut self, _: &crate::collector::SharedProcessData) -> Vec<AgentSession> {
                self.started.send(()).unwrap();
                self.finish.recv_timeout(Duration::from_secs(10)).unwrap();
                for session in &mut self.sessions {
                    session.subagent_counts.working = 2;
                }
                self.sessions.clone()
            }
        }
        impl crate::collector::StartupCollector for BatchCollector {}
        let project = tempfile::tempdir().unwrap();
        let mut first = waiting_session("codex");
        first.session_id = "first".into();
        first.cwd = project.path().to_string_lossy().into_owned();
        first.git_added = 999;
        let mut second = first.clone();
        second.session_id = "second".into();
        let mut done = first.clone();
        done.session_id = "done".into();
        done.status = SessionStatus::Done;
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let (published_tx, published_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut collector =
                crate::collector::MultiCollector::controlled(vec![Box::new(BatchCollector {
                    started: started_tx,
                    finish: finish_rx,
                    sessions: vec![first, second, done],
                })]);
            collector.collect_progressively(|session| {
                published_tx.send(session.clone()).unwrap();
            })
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            published_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        finish_tx.send(()).unwrap();
        for id in ["first", "second"] {
            let session = published_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(session.session_id, id);
            assert_eq!(session.subagent_counts.working, 2);
            assert_eq!(session.git_added, 0);
        }
        let sessions = worker.join().unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions
            .iter()
            .all(|s| s.git_added == 0 && s.subagent_counts.working == 2));
        assert!(
            published_rx.try_recv().is_err(),
            "Done sessions are not published"
        );
    }

    fn rate_limit(source: &str, pct: f64) -> RateLimitInfo {
        RateLimitInfo {
            source: source.to_string(),
            five_hour_pct: Some(pct),
            five_hour_resets_at: None,
            five_hour_window_minutes: Some(300),
            seven_day_pct: None,
            seven_day_resets_at: None,
            seven_day_window_minutes: None,
            updated_at: None,
        }
    }

    #[test]
    fn global_rates_share_category_deltas_and_expire_after_sessions_close() {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        let mut session = waiting_session("claude");
        session.total_input_tokens = 100_000;
        session.total_cache_read = 1_000_000;
        app.sessions.push(session);
        let start = Instant::now();
        app.update_session_token_rates(start);
        assert_eq!(app.global_token_rates(start), TokenRates::default());
        assert_eq!(app.token_rates.back(), Some(&0.0));
        assert_eq!(app.cache_read_rates.back(), Some(&0.0));

        app.sessions[0].total_input_tokens += 200;
        app.sessions[0].total_output_tokens += 100;
        app.sessions[0].total_cache_read += 600;
        app.sessions[0].total_cache_create += 200;
        app.sessions.push(app.sessions[0].clone());
        app.filter_text = "no matching sessions".into();
        app.stats_scope = StatsScope::AllLive;
        app.show_token_rate = false;
        app.update_session_token_rates(start + Duration::from_secs(2));
        let rates = app.global_token_rates(start + Duration::from_secs(2));
        assert_eq!(rates.total(), 500);
        assert_eq!(rates.cache_read, 600);
        assert_eq!(rates.cache_reuse_percent(), Some(60.0));
        assert_eq!(app.token_rates.back(), Some(&500.0));
        assert_eq!(app.cache_read_rates.back(), Some(&600.0));

        app.sessions.truncate(1);
        app.sessions[0].total_output_tokens += 50;
        app.update_session_token_rates(start + Duration::from_secs(19));
        app.sessions[0].total_cache_read += 200;
        app.update_session_token_rates(start + Duration::from_secs(61));
        assert_eq!(
            app.global_token_rates(start + Duration::from_secs(61))
                .total(),
            550
        );
        app.sessions.clear();
        app.update_session_token_rates(start + Duration::from_secs(62));
        let rates = app.global_token_rates(start + Duration::from_secs(62));
        assert_eq!(rates.total(), 50);
        assert_eq!(rates.cache_read, 200);
        assert_eq!(rates.cache_reuse_percent(), Some(100.0));
        assert_eq!(app.token_rates.back(), Some(&0.0));
        assert_eq!(app.cache_read_rates.back(), Some(&0.0));
        // Expire during rendering, without another collection tick.
        assert_eq!(
            app.global_token_rates(start + Duration::from_secs(121)),
            TokenRates::default()
        );
    }

    #[test]
    fn global_histories_handle_resets_discovery_gaps_and_manual_refreshes() {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        let start = Instant::now();
        app.sessions.push(waiting_session("codex"));
        app.sessions[0].total_cache_read = 1_000;
        app.update_session_token_rates(start);
        app.sessions[0].total_input_tokens = 100;
        app.sessions[0].total_cache_read = 10;
        app.update_session_token_rates(start + Duration::from_secs(2));
        assert_eq!(app.token_rates.back(), Some(&100.0));
        assert_eq!(app.cache_read_rates.back(), Some(&0.0));
        app.sessions[0].total_cache_read = 40;
        app.update_session_token_rates(start + Duration::from_secs(2));
        assert_eq!(app.cache_read_rates.back(), Some(&30.0));
        app.update_session_token_rates(start + Duration::from_secs(2));
        assert_eq!(app.cache_read_rates.back(), Some(&0.0));

        let mut session = app.sessions.pop().unwrap();
        app.update_session_token_rates(start + Duration::from_secs(3));
        session.total_cache_read += 20;
        app.sessions.push(session);
        app.update_session_token_rates(start + Duration::from_secs(20));
        assert_eq!(app.cache_read_rates.back(), Some(&20.0));
        assert_eq!(
            app.global_token_rates(start + Duration::from_secs(20))
                .cache_read,
            50
        );
        // A long collection gap establishes fresh baselines instead of a spike.
        app.sessions[0].total_input_tokens += 50_000;
        app.sessions[0].total_cache_read += 500_000;
        app.update_session_token_rates(start + Duration::from_secs(80));
        assert_eq!(
            app.global_token_rates(start + Duration::from_secs(80)),
            TokenRates::default()
        );
        for second in 81..=300 {
            app.update_session_token_rates(start + Duration::from_secs(second));
        }
        assert_eq!(app.token_rates.len(), GRAPH_HISTORY_LEN);
        assert_eq!(app.cache_read_rates.len(), GRAPH_HISTORY_LEN);
        assert!(app.token_rates.iter().all(|&rate| rate == 0.0));
        assert!(app.cache_read_rates.iter().all(|&rate| rate == 0.0));
        assert!(app.global_token_samples.is_empty());
    }

    #[test]
    fn reuse_is_weighted_by_input_tokens_and_excludes_output() {
        for (input, cache_read, cache_write, expected) in [
            (0, 0, 0, None),
            (200, 600, 200, Some(60.0)),
            (100, 0, 100, Some(0.0)),
            (0, 100, 0, Some(100.0)),
        ] {
            for output in [0, 10_000] {
                let rates = TokenRates {
                    input,
                    output,
                    cache_read,
                    cache_write,
                };
                assert_eq!(rates.cache_reuse_percent(), expected);
            }
        }
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        let now = Instant::now();
        app.record_global_token_delta(
            now,
            TokenRates {
                input: 100,
                ..Default::default()
            },
        );
        app.record_global_token_delta(
            now,
            TokenRates {
                cache_read: 900,
                ..Default::default()
            },
        );
        assert_eq!(
            app.global_token_rates(now).cache_reuse_percent(),
            Some(90.0)
        );
    }

    #[test]
    fn token_rate_uses_observed_deltas_and_expires_by_elapsed_time() {
        let start = Instant::now();
        let mut counter = TokenRateCounter::new(100_000);
        assert_eq!(counter.rate(start), 0);

        counter.update(100_010, start + Duration::from_secs(2));
        // No extrapolation to a full minute during startup.
        assert_eq!(counter.rate(start + Duration::from_secs(2)), 10);
        counter.update(100_080, start + Duration::from_secs(19));
        counter.update(100_110, start + Duration::from_secs(61));
        assert_eq!(counter.rate(start + Duration::from_secs(61)), 110);
        assert_eq!(counter.rate(start + Duration::from_secs(62)), 100);
        assert_eq!(counter.rate(start + Duration::from_secs(79)), 30);
        // Expiration also works between collection ticks.
        assert_eq!(counter.rate(start + Duration::from_secs(121)), 0);
        counter.update(100_110, start + Duration::from_secs(125));
        assert!(counter.samples.is_empty());
    }

    #[test]
    fn token_rates_reset_only_the_category_whose_counter_decreases() {
        let start = Instant::now();
        let mut session = waiting_session("claude");
        let mut rates = SessionTokenRates::new(&session, start);
        session.total_input_tokens = 100;
        session.total_output_tokens = 200;
        session.total_cache_read = 300;
        session.total_cache_create = 400;
        rates.update(&session, start + Duration::from_secs(2));
        assert_eq!(
            rates.rates(start + Duration::from_secs(2)),
            TokenRates {
                input: 100,
                output: 200,
                cache_read: 300,
                cache_write: 400,
            }
        );

        session.total_cache_read = 10;
        rates.update(&session, start + Duration::from_secs(4));
        assert_eq!(
            rates.rates(start + Duration::from_secs(4)),
            TokenRates {
                input: 100,
                output: 200,
                cache_read: 0,
                cache_write: 400,
            }
        );
        session.total_cache_read = 25;
        rates.update(&session, start + Duration::from_secs(6));
        assert_eq!(rates.rates(start + Duration::from_secs(6)).cache_read, 15);
    }

    #[test]
    fn token_rates_follow_session_identity_across_selection_and_reordering() {
        let start = Instant::now();
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        app.sessions = vec![
            waiting_session("claude"),
            waiting_session("codex"),
            waiting_session("opencode"),
            waiting_session("claude"),
        ];
        app.sessions[3].session_id = "another-session".into();
        app.update_session_token_rates(start);
        for (index, session) in app.sessions.iter_mut().enumerate() {
            add_model_usage(session, "shared-model", [(index as u64 + 1) * 100, 0, 0, 0]);
        }
        let now = start + Duration::from_secs(2);
        app.update_session_token_rates(now);
        for selected in [3, 1, 0, 2] {
            app.selected = selected;
            assert_eq!(
                app.model_token_rate(&app.sessions[app.selected], "shared-model", now),
                (selected as u64 + 1) * 100
            );
            assert_eq!(
                app.session_token_rates(&app.sessions[app.selected], now)
                    .input,
                (selected as u64 + 1) * 100
            );
        }
        app.sessions.reverse();
        app.update_session_token_rates(start + Duration::from_secs(4));
        for session in &app.sessions {
            assert_eq!(
                app.model_token_rate(session, "shared-model", start + Duration::from_secs(4)),
                session.total_input_tokens
            );
            assert_eq!(
                app.session_token_rates(session, start + Duration::from_secs(4))
                    .input,
                session.total_input_tokens
            );
        }
    }

    #[test]
    fn token_rates_keep_briefly_missing_sessions_and_discard_stale_baselines() {
        let start = Instant::now();
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        let mut session = waiting_session("claude");
        add_model_usage(&mut session, "shared-model", [1_000, 0, 0, 0]);
        app.sessions = vec![session.clone()];
        app.update_session_token_rates(start);
        assert_eq!(
            app.session_token_rates(&session, start),
            TokenRates::default()
        );
        assert_eq!(app.model_token_rate(&session, "shared-model", start), 0);
        add_model_usage(&mut app.sessions[0], "shared-model", [200, 0, 0, 0]);
        app.update_session_token_rates(start + Duration::from_secs(2));
        app.sessions.clear();
        app.update_session_token_rates(start + Duration::from_secs(4));
        assert_eq!(app.session_token_rates.len(), 1);

        add_model_usage(&mut session, "shared-model", [250, 0, 0, 0]);
        app.sessions.push(session.clone());
        app.update_session_token_rates(start + Duration::from_secs(20));
        assert_eq!(
            app.model_token_rate(&session, "shared-model", start + Duration::from_secs(20)),
            250
        );
        assert_eq!(
            app.session_token_rates(&session, start + Duration::from_secs(20))
                .input,
            250
        );
        app.sessions.clear();
        app.update_session_token_rates(start + Duration::from_secs(79));
        assert_eq!(app.session_token_rates.len(), 1);
        app.update_session_token_rates(start + Duration::from_secs(80));
        assert!(app.session_token_rates.is_empty());

        add_model_usage(&mut session, "shared-model", [48_750, 0, 0, 0]);
        app.sessions.push(session.clone());
        app.update_session_token_rates(start + Duration::from_secs(81));
        assert_eq!(
            app.model_token_rate(&session, "shared-model", start + Duration::from_secs(81)),
            0
        );
        assert_eq!(
            app.session_token_rates(&session, start + Duration::from_secs(81)),
            TokenRates::default()
        );
    }

    #[test]
    fn test_rate_limited_promotion_is_per_agent_cli() {
        // Claude is saturated, Codex is not. Only the Claude session should
        // be promoted.
        let mut sessions = vec![waiting_session("claude"), waiting_session("codex")];
        let limits = vec![rate_limit("claude", 95.0)];
        promote_waiting_to_rate_limited(&mut sessions, &limits);
        assert_eq!(sessions[0].status, SessionStatus::RateLimited);
        assert_eq!(sessions[1].status, SessionStatus::Waiting);
    }

    #[test]
    fn test_rate_limited_promotion_ignores_below_threshold() {
        let mut sessions = vec![waiting_session("claude")];
        let limits = vec![rate_limit("claude", 89.9)];
        promote_waiting_to_rate_limited(&mut sessions, &limits);
        assert_eq!(sessions[0].status, SessionStatus::Waiting);
    }

    #[test]
    fn test_rate_limited_promotion_skips_non_waiting_sessions() {
        let mut sessions = vec![waiting_session("claude")];
        sessions[0].status = SessionStatus::Thinking;
        let limits = vec![rate_limit("claude", 99.0)];
        promote_waiting_to_rate_limited(&mut sessions, &limits);
        assert_eq!(sessions[0].status, SessionStatus::Thinking);
    }

    #[test]
    fn supported_agent_command_accepts_opencode() {
        assert!(is_supported_agent_command("/usr/local/bin/claude"));
        assert!(is_supported_agent_command("codex --resume abc"));
        assert!(is_supported_agent_command("/opt/homebrew/bin/opencode"));
        assert!(!is_supported_agent_command("node server.js"));
    }

    #[test]
    fn killable_agent_command_rejects_codex_app_server() {
        assert!(is_killable_agent_command("codex --resume abc"));
        assert!(is_killable_agent_command("/usr/local/bin/claude"));
        assert!(!is_killable_agent_command(
            "/Applications/Codex.app/Contents/Resources/codex app-server --analytics-default-enabled"
        ));
    }
}
