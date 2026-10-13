//! Collection ownership moves to a temporary startup worker, then back to App.
use crate::collector::{read_rate_limits, McpServer, MultiCollector};
use crate::host_info::{HostMetrics, HostSampler};
use crate::model::{AgentSession, OrphanPort, RateLimitInfo};
use std::path::PathBuf;

pub(super) struct CollectionState {
    pub collector: MultiCollector,
    host_sampler: HostSampler,
    rate_limit_counter: u32,
    rate_limits: Vec<RateLimitInfo>,
}

pub(super) struct Collected {
    pub sessions: Vec<AgentSession>,
    pub orphan_ports: Vec<OrphanPort>,
    pub mcp_servers: Vec<McpServer>,
    pub host_metrics: Option<HostMetrics>,
    pub rate_limits: Vec<RateLimitInfo>,
}

pub(super) enum StartupMessage {
    Session(Box<AgentSession>),
    Complete(Box<(CollectionState, Collected)>),
}

impl CollectionState {
    pub fn new(hidden: &[String], directories: &[PathBuf]) -> Self {
        Self {
            collector: MultiCollector::with_hidden_and_claude_config_dirs(hidden, directories),
            host_sampler: HostSampler::new(),
            rate_limit_counter: 5,
            rate_limits: Vec::new(),
        }
    }

    pub fn collect(&mut self) -> Collected {
        let sessions = self.collector.collect();
        self.finish(sessions)
    }

    pub fn collect_progressively(&mut self, publish: impl FnMut(&AgentSession)) -> Collected {
        let sessions = self.collector.collect_progressively(publish);
        self.finish(sessions)
    }

    fn finish(&mut self, sessions: Vec<AgentSession>) -> Collected {
        if self.rate_limits.is_empty() || self.rate_limit_counter >= 5 {
            self.rate_limit_counter = 0;
            self.rate_limits = read_rate_limits(&self.collector.all_config_dirs());
            self.rate_limits.extend(self.collector.agent_rate_limits());
        } else {
            self.rate_limit_counter += 1;
        }
        Collected {
            sessions,
            orphan_ports: self.collector.orphan_ports.clone(),
            mcp_servers: self.collector.mcp_servers.clone(),
            host_metrics: self.host_sampler.sample(),
            rate_limits: self.rate_limits.clone(),
        }
    }
}
