use super::{App, StatsScope};
use crate::model::{AgentSession, AgentTotals, SessionStatus};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

pub(crate) const HISTORY_WINDOW: Duration = Duration::from_secs(15 * 60);
type SessionKey = (&'static str, String, String);

fn key(s: &AgentSession) -> SessionKey {
    (s.agent_cli, s.config_root.clone(), s.session_id.clone())
}

fn live(s: &AgentSession) -> bool {
    s.pid != 0 && !matches!(s.status, SessionStatus::Done | SessionStatus::Unknown)
}

struct Sample {
    at: Instant,
    total: AgentTotals,
    sessions: HashMap<SessionKey, AgentTotals>,
}

#[derive(Default)]
pub(crate) struct AgentHistory {
    samples: VecDeque<Sample>,
}

impl App {
    fn live_agent_sessions(&self) -> impl Iterator<Item = &AgentSession> {
        let mut seen = HashSet::new();
        self.sessions.iter().filter(move |s| {
            live(s)
                && matches!(s.agent_cli, "claude" | "codex")
                && seen.insert((s.agent_cli, s.config_root.as_str(), s.session_id.as_str()))
        })
    }

    pub fn toggle_agent_scope(&mut self) {
        self.agent_scope = match self.agent_scope {
            StatsScope::AllLive => StatsScope::Selected,
            StatsScope::Selected => StatsScope::AllLive,
        };
        if let Err(error) = (self.save_agent_scope)(self.agent_scope) {
            self.set_status(format!("agent scope save failed: {error}"));
        }
    }

    /// Global snapshot scope never depends on selection, search or panel visibility.
    pub(crate) fn global_subagent_counts(&self) -> AgentTotals {
        let mut totals = AgentTotals::default();
        for s in self.live_agent_sessions() {
            totals.add_subagents(s);
        }
        totals
    }

    /// Each confirmed live root contributes once, including when it has no children.
    pub(crate) fn global_agent_counts(&self) -> AgentTotals {
        let mut totals = AgentTotals::default();
        for s in self.live_agent_sessions() {
            totals.add_session(s);
        }
        totals
    }

    pub(crate) fn displayed_agent_counts(&self) -> AgentTotals {
        if self.agent_scope == StatsScope::AllLive {
            return self.global_agent_counts();
        }
        let mut totals = AgentTotals::default();
        if let Some(s) = self.sessions.get(self.selected).filter(|s| live(s)) {
            totals.add_session(s);
        }
        totals
    }

    pub(crate) fn record_agents(&mut self, now: Instant) {
        let mut sessions = HashMap::new();
        let mut total = AgentTotals::default();
        for s in self.live_agent_sessions() {
            let mut counts = AgentTotals::default();
            counts.add_session(s);
            sessions.insert(key(s), counts);
            total.add_session(s);
        }
        let history = &mut self.agent_history.samples;
        while history
            .front()
            .is_some_and(|s| now.duration_since(s.at) >= HISTORY_WINDOW)
        {
            history.pop_front();
        }
        if let Some(last) = history
            .back_mut()
            .filter(|s| now.duration_since(s.at) < Duration::from_secs(2))
        {
            last.total = total;
            last.sessions = sessions;
        } else {
            history.push_back(Sample {
                at: now,
                total,
                sessions,
            });
        }
    }

    /// Pick one real sample per column; independent provider maxima would invent concurrency.
    pub(crate) fn agent_bars(&self, columns: usize, now: Instant) -> Vec<Option<AgentTotals>> {
        let mut bars: Vec<Option<AgentTotals>> = vec![None; columns];
        let selected = self.sessions.get(self.selected).map(key);
        for sample in &self.agent_history.samples {
            let age = now.saturating_duration_since(sample.at);
            if age >= HISTORY_WINDOW || columns == 0 {
                continue;
            }
            let value = match self.agent_scope {
                StatsScope::AllLive => Some(sample.total),
                StatsScope::Selected => selected
                    .as_ref()
                    .and_then(|k| sample.sessions.get(k))
                    .copied(),
            };
            let Some(value) = value else {
                continue;
            };
            let column = columns
                - 1
                - ((age.as_secs_f64() / HISTORY_WINDOW.as_secs_f64() * columns as f64) as usize)
                    .min(columns - 1);
            if bars[column].is_none_or(|previous| value.working() >= previous.working()) {
                bars[column] = Some(value);
            }
        }
        bars
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::PanelVisibility, theme::Theme};

    fn app() -> App {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.agent_history.samples.clear();
        app.sessions
            .retain(|s| matches!(s.agent_cli, "claude" | "codex"));
        for s in &mut app.sessions {
            s.subagent_counts = Default::default();
            s.status = SessionStatus::Waiting;
        }
        app
    }

    #[test]
    fn working_main_agent_counts_without_children_in_both_scopes_and_history() {
        let mut app = app();
        let index = app
            .sessions
            .iter()
            .position(|s| s.agent_cli == "codex")
            .unwrap();
        app.selected = index;
        app.sessions[index].status = SessionStatus::Thinking;
        let now = Instant::now();
        app.record_agents(now);
        for scope in [StatsScope::AllLive, StatsScope::Selected] {
            app.agent_scope = scope;
            assert_eq!(app.displayed_agent_counts().codex.working, 1);
            assert_eq!(app.agent_bars(1, now)[0].unwrap().codex.working, 1);
        }
        let snapshot = app.to_snapshot(2000);
        assert_eq!(snapshot.agent_counts.codex.working, 1);
        assert_eq!(snapshot.subagent_counts.codex.working, 0);
    }

    #[test]
    fn main_and_child_activity_add_independently_without_duplicate_roots() {
        let mut app = app();
        app.sessions.truncate(1);
        app.sessions[0].subagent_counts.working = 2;
        app.sessions[0].subagent_counts.unknown = 1;
        for (status, expected) in [
            (SessionStatus::Thinking, 3),
            (SessionStatus::Executing, 3),
            (SessionStatus::Background, 2),
            (SessionStatus::Waiting, 2),
            (SessionStatus::RateLimited, 2),
            (SessionStatus::Done, 0),
            (SessionStatus::Unknown, 0),
        ] {
            app.sessions.truncate(1);
            app.sessions[0].status = status;
            app.sessions.push(app.sessions[0].clone());
            assert_eq!(app.global_agent_counts().claude.working, expected);
            for scope in [StatsScope::Selected, StatsScope::AllLive] {
                app.agent_scope = scope;
                assert_eq!(app.displayed_agent_counts().claude.working, expected);
            }
        }
        app.sessions.truncate(1);
        app.sessions[0].status = SessionStatus::Executing;
        assert_eq!(app.global_agent_counts().unknown(), 1);
        app.sessions[0].pid = 0;
        assert_eq!(app.global_agent_counts().working(), 0);
    }

    #[test]
    fn main_agent_activity_changes_are_recorded_without_erasing_prior_peaks() {
        let mut app = app();
        app.sessions.truncate(1);
        let start = Instant::now();
        app.sessions[0].status = SessionStatus::Executing;
        app.record_agents(start);
        app.sessions[0].status = SessionStatus::Waiting;
        app.record_agents(start + Duration::from_secs(2));
        assert_eq!(app.displayed_agent_counts().working(), 0);
        assert_eq!(
            app.agent_bars(1, start + Duration::from_secs(2))[0]
                .unwrap()
                .working(),
            1
        );
    }

    #[test]
    fn snapshots_use_complete_global_counts_regardless_of_scope_or_detail_cap() {
        let mut app = app();
        app.sessions[0].subagent_counts.working = 30;
        app.sessions[0].subagent_counts.unknown = 2;
        app.agent_scope = StatsScope::Selected;
        app.selected = 1;
        let snapshot = app.to_snapshot(2000);
        assert_eq!(snapshot.subagent_counts.claude.working, 30);
        assert_eq!(snapshot.sessions[0].subagent_counts.unknown, 2);
        assert!(snapshot.sessions[0].subagents.len() <= 16);
        assert_eq!(app.displayed_agent_counts().working(), 0);
    }

    #[test]
    fn compression_uses_simultaneous_counts_and_history_survives_departure() {
        let mut app = app();
        let start = Instant::now();
        app.sessions[0].subagent_counts.working = 5;
        app.record_agents(start);
        app.sessions[0].subagent_counts.working = 0;
        app.sessions
            .iter_mut()
            .find(|s| s.agent_cli == "codex")
            .unwrap()
            .subagent_counts
            .working = 4;
        app.record_agents(start + Duration::from_secs(2));
        let bar = app.agent_bars(1, start + Duration::from_secs(2))[0].unwrap();
        assert_eq!(bar.working(), 5);
        assert_eq!(bar.codex.working, 0);
        app.sessions.clear();
        app.record_agents(start + Duration::from_secs(4));
        assert_eq!(app.global_subagent_counts().working(), 0);
        assert_eq!(
            app.agent_bars(1, start + Duration::from_secs(4))[0]
                .unwrap()
                .working(),
            5
        );
        app.record_agents(start + HISTORY_WINDOW + Duration::from_secs(4));
        assert_eq!(app.agent_history.samples.len(), 1);
        assert_eq!(
            app.agent_bars(1, start + HISTORY_WINDOW + Duration::from_secs(4))[0]
                .unwrap()
                .working(),
            0
        );
    }

    #[test]
    fn selected_history_follows_identity_and_collection_ignores_visibility_and_search() {
        let mut app = app();
        let start = Instant::now();
        app.show_agents = false;
        app.filter_text = "no matches".into();
        app.sessions[0].subagent_counts.working = 3;
        app.record_agents(start);
        app.agent_scope = StatsScope::Selected;
        assert_eq!(app.agent_bars(1, start)[0].unwrap().working(), 3);
        app.sessions.swap(0, 1);
        app.selected = 1;
        assert_eq!(app.agent_bars(1, start)[0].unwrap().working(), 3);
        app.selected = 0;
        assert_eq!(app.agent_bars(1, start)[0].unwrap().working(), 0);
        assert!(app.agent_bars(100, start).iter().any(Option::is_none));
    }

    #[test]
    fn duplicates_manual_refresh_and_unconfirmed_roots_do_not_inflate_counts() {
        let mut app = app();
        let start = Instant::now();
        app.sessions[0].subagent_counts.working = 2;
        app.sessions.push(app.sessions[0].clone());
        app.record_agents(start);
        assert_eq!(app.global_subagent_counts().working(), 2);
        app.record_agents(start + Duration::from_millis(500));
        assert_eq!(app.agent_history.samples.len(), 1);
        for s in &mut app.sessions {
            s.status = SessionStatus::Unknown;
        }
        assert_eq!(app.global_subagent_counts().working(), 0);
        app.record_agents(start + Duration::from_secs(3));
        assert_eq!(app.agent_history.samples.len(), 2);
    }

    #[test]
    fn history_and_current_counts_use_the_same_root_per_profile() {
        let mut app = app();
        app.sessions.truncate(1);
        app.sessions[0].subagent_counts.working = 2;
        let mut duplicate = app.sessions[0].clone();
        duplicate.subagent_counts.working = 9;
        app.sessions.push(duplicate);
        let mut other_profile = app.sessions[0].clone();
        other_profile.config_root.push_str("-other");
        other_profile.subagent_counts.working = 3;
        app.sessions.push(other_profile);

        let now = Instant::now();
        app.record_agents(now);
        assert_eq!(app.global_subagent_counts().working(), 5);
        assert_eq!(app.global_agent_counts().working(), 5);
        assert_eq!(app.agent_bars(1, now)[0].unwrap().working(), 5);
        app.agent_scope = StatsScope::Selected;
        assert_eq!(app.displayed_agent_counts().working(), 2);
        assert_eq!(app.agent_bars(1, now)[0].unwrap().working(), 2);
        app.selected = 2;
        assert_eq!(app.agent_bars(1, now)[0].unwrap().working(), 3);
    }
}
