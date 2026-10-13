use super::{App, TokenRates};
use crate::model::{AgentSession, ModelUsage, SessionStatus};
use std::collections::HashSet;
use std::time::Instant;

/// The sessions whose usage is displayed in the Tokens and Models panes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatsScope {
    #[default]
    Selected,
    AllLive,
}

impl StatsScope {
    pub(crate) fn config_value(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::AllLive => "all_live",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Selected => "Selected",
            Self::AllLive => "All live",
        }
    }
}

/// Both panes use this calculation so their totals and model shares agree.
#[derive(Default)]
pub(crate) struct UsageStats {
    pub session_count: usize,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub turns: u64,
    pub model_usage: Vec<ModelUsage>,
    pub rates: TokenRates,
}

impl UsageStats {
    pub fn total_tokens(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }

    pub fn average_tokens(&self) -> u64 {
        self.total_tokens().checked_div(self.turns).unwrap_or(0)
    }
}

impl App {
    pub fn toggle_stats_scope(&mut self) {
        self.stats_scope = match self.stats_scope {
            StatsScope::Selected => StatsScope::AllLive,
            StatsScope::AllLive => StatsScope::Selected,
        };
        if let Err(error) = (self.save_stats_scope)(self.stats_scope) {
            self.set_status(format!(
                "stats: {} (save failed: {error})",
                self.stats_scope.label()
            ));
        }
    }

    /// Use collector-confirmed liveness, independently of list search/selection.
    /// Session totals already include any subagent usage tracked by the collector.
    pub(crate) fn live_sessions(&self) -> impl Iterator<Item = &AgentSession> {
        let mut seen = HashSet::new();
        self.sessions.iter().filter(move |session| {
            session.pid != 0
                && matches!(
                    session.status,
                    SessionStatus::Thinking
                        | SessionStatus::Executing
                        | SessionStatus::Waiting
                        | SessionStatus::RateLimited
                )
                && seen.insert((session.agent_cli, session.session_id.as_str()))
        })
    }

    pub(crate) fn live_token_rates(&self, now: Instant) -> TokenRates {
        let mut rates = TokenRates::default();
        for session in self.live_sessions() {
            rates.add(&self.session_token_rates(session, now));
        }
        rates
    }

    pub(crate) fn usage_stats(&self, now: Instant) -> UsageStats {
        let mut stats = UsageStats::default();
        let sessions: Box<dyn Iterator<Item = &AgentSession> + '_> = match self.stats_scope {
            StatsScope::Selected => Box::new(self.sessions.get(self.selected).into_iter()),
            StatsScope::AllLive => Box::new(self.live_sessions()),
        };
        for session in sessions {
            stats.session_count += 1;
            stats.input += session.total_input_tokens;
            stats.output += session.total_output_tokens;
            stats.cache_read += session.total_cache_read;
            stats.cache_write += session.total_cache_create;
            stats.turns += u64::from(session.turn_count);
            ModelUsage::merge(&mut stats.model_usage, &session.model_usage);
            stats.rates.add(&self.session_token_rates(session, now));
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use std::time::Duration;

    fn fixture() -> App {
        let mut app = App::new_with_config(Theme::default(), &[], Default::default());
        crate::demo::populate_demo(&mut app);
        app.live_token_history.clear();
        app.sessions.truncate(3);
        for (session, (cli, amounts, turns, status)) in app.sessions.iter_mut().zip([
            ("claude", [100, 20, 300, 40], 2, SessionStatus::Thinking),
            ("codex", [40, 30, 20, 10], 3, SessionStatus::Waiting),
            ("opencode", [10, 10, 10, 10], 5, SessionStatus::RateLimited),
        ]) {
            session.agent_cli = cli;
            session.session_id = cli.to_string();
            [
                session.total_input_tokens,
                session.total_output_tokens,
                session.total_cache_read,
                session.total_cache_create,
            ] = amounts;
            session.turn_count = turns;
            session.status = status;
            session.model_usage.clear();
        }
        ModelUsage::add(&mut app.sessions[0].model_usage, "shared", 400);
        ModelUsage::add(&mut app.sessions[0].model_usage, "Unknown", 60);
        ModelUsage::add(&mut app.sessions[1].model_usage, "shared", 50);
        ModelUsage::add(&mut app.sessions[1].model_usage, "shared-v2", 50);
        ModelUsage::add(&mut app.sessions[2].model_usage, "provider/shared", 40);
        app
    }

    #[test]
    fn live_usage_shares_membership_and_preserves_collector_accounting() {
        let mut app = fixture();
        app.stats_scope = StatsScope::AllLive;
        // Existing Claude subagent totals are already rolled up by its collector.
        assert!(!app.sessions[0].subagents.is_empty());
        app.sessions.push(app.sessions[0].clone());
        for status in [SessionStatus::Done, SessionStatus::Unknown] {
            let mut excluded = app.sessions[0].clone();
            excluded.session_id = format!("{status:?}");
            excluded.status = status;
            app.sessions.push(excluded);
        }
        let mut unowned = app.sessions[0].clone();
        unowned.session_id = "no-pid".into();
        unowned.pid = 0;
        app.sessions.push(unowned);

        let stats = app.usage_stats(Instant::now());
        assert_eq!(stats.session_count, 3);
        assert_eq!(
            [
                stats.input,
                stats.output,
                stats.cache_read,
                stats.cache_write
            ],
            [150, 60, 330, 60]
        );
        assert_eq!(stats.total_tokens(), 600);
        assert_eq!(stats.turns, 10);
        assert_eq!(stats.average_tokens(), 60);
        assert_eq!(
            stats
                .model_usage
                .iter()
                .map(|entry| entry.total_tokens)
                .sum::<u64>(),
            600
        );
        let sorted = ModelUsage::sorted(&stats.model_usage);
        assert_eq!((&*sorted[0].model, sorted[0].total_tokens), ("shared", 450));
        assert_eq!(sorted.len(), 4);

        app.filter_text = "no matching session".into();
        assert!(app.visible_indices().is_empty());
        app.selected = 1;
        assert_eq!(app.usage_stats(Instant::now()).total_tokens(), 600);
        app.stats_scope = StatsScope::Selected;
        assert_eq!(app.usage_stats(Instant::now()).total_tokens(), 100);
    }

    #[test]
    fn live_rate_history_tracks_churn_without_lifetime_spikes() {
        let mut app = fixture();
        let start = Instant::now();
        app.update_session_token_rates(start);
        assert_eq!(app.live_token_history.back(), Some(&0));
        for (i, session) in app.sessions.iter_mut().enumerate() {
            let factor = i as u64 + 1;
            session.total_input_tokens += factor;
            session.total_output_tokens += 2 * factor;
            session.total_cache_read += 3 * factor;
            session.total_cache_create += 4 * factor;
        }
        // Multiple discovery rows for the same session must not double rates.
        app.sessions.push(app.sessions[0].clone());
        let now = start + Duration::from_secs(2);
        app.update_session_token_rates(now);
        app.stats_scope = StatsScope::AllLive;
        assert_eq!(
            app.usage_stats(now).rates,
            TokenRates {
                input: 6,
                output: 12,
                cache_read: 18,
                cache_write: 24,
            }
        );
        assert_eq!(
            app.live_token_history.iter().copied().collect::<Vec<_>>(),
            vec![0, 60]
        );

        app.sessions.retain(|session| session.agent_cli != "claude");
        let now = start + Duration::from_secs(4);
        app.update_session_token_rates(now);
        assert_eq!(app.usage_stats(now).total_tokens(), 190);
        assert_eq!(app.live_token_history.back(), Some(&50));
        // Past samples keep their membership from the time they were observed.
        assert_eq!(app.live_token_history[1], 60);

        let mut arriving = app.sessions[0].clone();
        arriving.session_id = "new-session".into();
        arriving.total_input_tokens = 1_000_000;
        app.sessions.push(arriving);
        let now = start + Duration::from_secs(6);
        app.update_session_token_rates(now);
        assert_eq!(app.live_token_history.back(), Some(&50));
        app.update_session_token_rates(start + Duration::from_secs(62));
        assert_eq!(app.live_token_history.back(), Some(&0));
        app.sessions.clear();
        app.update_session_token_rates(start + Duration::from_secs(64));
        assert_eq!(
            app.usage_stats(start + Duration::from_secs(64))
                .session_count,
            0
        );
    }

    #[test]
    fn history_is_bounded_and_collected_while_selected() {
        let mut app = fixture();
        let start = Instant::now();
        for tick in 0..205 {
            app.sessions[0].total_input_tokens += 1;
            app.update_session_token_rates(start + Duration::from_secs(tick * 2));
        }
        assert_eq!(app.stats_scope, StatsScope::Selected);
        assert_eq!(app.live_token_history.len(), 200);
        assert_eq!(app.live_token_history.back(), Some(&30));
    }

    #[test]
    fn scope_save_failure_preserves_the_in_memory_choice() {
        let mut app = fixture();
        app.save_stats_scope = |_| Err("read-only config".to_string());
        app.toggle_stats_scope();
        assert_eq!(app.stats_scope, StatsScope::AllLive);
        assert!(app
            .status_msg
            .as_ref()
            .unwrap()
            .0
            .contains("save failed: read-only config"));
        app.save_stats_scope = |_| Ok(());
        app.toggle_stats_scope();
        assert_eq!(app.stats_scope, StatsScope::Selected);
    }
}
