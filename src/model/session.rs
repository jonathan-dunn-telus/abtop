use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Type of file operation performed by the agent.
#[derive(Debug, Clone, PartialEq)]
pub enum FileOp {
    Read,
    Write,
    Edit,
}

impl fmt::Display for FileOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileOp::Read => write!(f, "R"),
            FileOp::Write => write!(f, "W"),
            FileOp::Edit => write!(f, "E"),
        }
    }
}

/// A single file access event recorded from agent tool usage.
#[derive(Debug, Clone)]
pub struct FileAccess {
    pub path: String,
    pub operation: FileOp,
    #[allow(dead_code)]
    pub turn_index: u32,
}

/// Maximum file access entries kept per session to bound memory.
pub const MAX_FILE_ACCESSES: usize = 1000;

/// Account-level rate limit info (shared across all sessions).
#[derive(Debug, Clone, Default, Serialize)]
pub struct RateLimitInfo {
    /// "claude" or "codex"
    pub source: String,
    /// 5-hour window usage percentage (0-100)
    pub five_hour_pct: Option<f64>,
    /// 5-hour window reset timestamp (epoch seconds)
    pub five_hour_resets_at: Option<u64>,
    /// 5-hour slot duration in minutes, when reported by the source.
    pub five_hour_window_minutes: Option<u64>,
    /// 7-day window usage percentage (0-100)
    ///
    /// Historical field name kept for compatibility; Codex may use this slot
    /// for a longer account-level window such as 30 days.
    pub seven_day_pct: Option<f64>,
    /// 7-day window reset timestamp (epoch seconds)
    pub seven_day_resets_at: Option<u64>,
    /// Long-window slot duration in minutes, when reported by the source.
    pub seven_day_window_minutes: Option<u64>,
    /// When this data was last updated
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum SessionStatus {
    /// Model is generating a response (last_user_ts_ms > 0)
    Thinking,
    /// Running a tool (descendant CPU active OR current_task non-empty)
    Executing,
    /// Idle, waiting for user input or permission prompt
    Waiting,
    /// Session appears recent, but process ownership is not confirmed
    Unknown,
    /// Waiting due to rate limit
    RateLimited,
    /// Session finished
    Done,
}

impl SessionStatus {
    /// Returns true for states where the agent is actively doing work.
    pub fn is_active(&self) -> bool {
        matches!(self, SessionStatus::Thinking | SessionStatus::Executing)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChildProcess {
    pub pid: u32,
    pub command: String,
    pub mem_kb: u64,
    pub port: Option<u16>,
}

/// A port left open by a process whose parent session has ended.
#[derive(Debug, Clone, Serialize)]
pub struct OrphanPort {
    pub port: u16,
    pub pid: u32,
    pub command: String,
    pub project_name: String,
}

#[derive(Debug, Clone)]
pub struct SubAgent {
    /// Relative transcript identity; not exposed in snapshots.
    pub activity_key: String,
    pub name: String,
    pub status: String,
    pub tokens: u64,
}

/// Current agent activity, independent of lifetime token accounting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AgentCounts {
    pub working: u32,
    pub unknown: u32,
}

impl AgentCounts {
    pub fn add(&mut self, other: Self) {
        self.working = self.working.saturating_add(other.working);
        self.unknown = self.unknown.saturating_add(other.unknown);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct AgentTotals {
    pub claude: AgentCounts,
    pub codex: AgentCounts,
}

impl AgentTotals {
    pub fn working(self) -> u32 {
        self.claude.working.saturating_add(self.codex.working)
    }
    pub fn unknown(self) -> u32 {
        self.claude.unknown.saturating_add(self.codex.unknown)
    }
    pub fn add_subagents(&mut self, session: &AgentSession) {
        match session.agent_cli {
            "claude" => self.claude.add(session.subagent_counts),
            "codex" => self.codex.add(session.subagent_counts),
            _ => {}
        }
    }

    /// Chart activity includes the working root as well as its working children.
    pub fn add_session(&mut self, session: &AgentSession) {
        let counts = match session.agent_cli {
            "claude" => &mut self.claude,
            "codex" => &mut self.codex,
            _ => return,
        };
        counts.add(session.subagent_counts);
        counts.working = counts
            .working
            .saturating_add(u32::from(session.status.is_active()));
    }
}

/// A single tool invocation from a session transcript.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Tool name: "Read", "Edit", "Bash", "Write", "Grep", "Glob", "Agent", etc.
    pub name: String,
    /// Short argument (file path, command prefix, pattern).
    pub arg: String,
    /// Duration in milliseconds (0 if unknown).
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// A compact, redacted chat line from the session transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

/// Maximum chat messages kept per session to bound memory and UI noise.
pub const MAX_CHAT_MESSAGES: usize = 12;

/// Which surface launched this session's process, detected from the resolved
/// executable path in its command line. Always `Cli` for Codex and OpenCode
/// sessions — no desktop-app or editor-extension equivalent is known for
/// those tools yet.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum LaunchSurface {
    /// Plain CLI invocation: a terminal shell running an npm/homebrew/native
    /// install (or the auto-updater's `versions/<ver>` layout).
    Cli,
    /// The Claude desktop app's bundled `claude-code` binary.
    App,
    /// An editor extension (VS Code, Cursor, Windsurf, ...).
    Ide,
}

impl LaunchSurface {
    /// Single-char suffix appended to the agent-label column in the sessions
    /// table. Empty for the default `Cli` case so existing rows/tests are
    /// unaffected; `a`/`i` flag the less common surfaces.
    pub fn label_suffix(&self) -> &'static str {
        match self {
            LaunchSurface::Cli => "",
            LaunchSurface::App => "a",
            LaunchSurface::Ide => "i",
        }
    }
}

/// Lifetime tokens attributed to one model, using the session's additive
/// input/output/cache accounting. Missing model metadata uses `Unknown`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelUsage {
    pub model: String,
    pub total_tokens: u64,
}

impl ModelUsage {
    pub(crate) fn add(entries: &mut Vec<Self>, model: &str, total_tokens: u64) {
        if total_tokens == 0 {
            return;
        }
        let model = match model.trim() {
            "" | "-" => "Unknown",
            model => model,
        };
        if let Some(entry) = entries.iter_mut().find(|entry| entry.model == model) {
            entry.total_tokens += total_tokens;
        } else {
            entries.push(Self {
                model: model.to_string(),
                total_tokens,
            });
        }
    }

    pub(crate) fn merge(entries: &mut Vec<Self>, additions: &[Self]) {
        for entry in additions {
            Self::add(entries, &entry.model, entry.total_tokens);
        }
    }

    pub(crate) fn sorted(entries: &[Self]) -> Vec<Self> {
        let mut entries = entries.to_vec();
        entries.sort_unstable_by(|a, b| {
            b.total_tokens
                .cmp(&a.total_tokens)
                .then_with(|| a.model.cmp(&b.model))
        });
        entries
    }
}

#[derive(Debug, Clone)]
pub struct AgentSession {
    /// Which CLI tool this session belongs to: "claude", "codex", etc.
    /// Also used as the identifier for the `hidden_agents` config key
    /// (case-insensitive match).
    pub agent_cli: &'static str,
    /// Which surface launched this session's process (CLI / desktop app / IDE extension).
    pub launch_surface: LaunchSurface,
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    pub project_name: String,
    pub started_at: u64,
    pub status: SessionStatus,
    pub model: String,
    /// Reasoning effort setting (Codex CLI only: "minimal" | "low" | "medium" | "high").
    /// Empty string when unknown or not applicable.
    pub effort: String,
    pub context_percent: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read: u64,
    pub total_cache_create: u64,
    /// Per-model usage for the same lifetime and subagent scope as the totals.
    pub model_usage: Vec<ModelUsage>,
    pub turn_count: u32,
    pub current_tasks: Vec<String>,
    pub mem_mb: u64,
    pub version: String,
    pub git_branch: String,
    pub git_added: u32,
    pub git_modified: u32,
    pub token_history: Vec<u64>,
    /// Per-turn context size (input tokens) for context evolution visualization.
    pub context_history: Vec<u64>,
    /// Number of detected compaction events (context dropped > 30% between turns).
    pub compaction_count: u32,
    /// Context window size for this session's model (e.g. 200K, 1M).
    pub context_window: u64,
    pub subagents: Vec<SubAgent>,
    pub subagent_counts: AgentCounts,
    pub mem_file_count: u32,
    pub mem_line_count: u32,
    pub children: Vec<ChildProcess>,
    /// First user prompt text, truncated — used as session title
    pub initial_prompt: String,
    /// First assistant response text (text blocks only) — used as summary fallback
    pub first_assistant_text: String,
    /// Recent user/assistant chat tail, excluding tool results and tool inputs.
    pub chat_messages: Vec<ChatMessage>,
    /// Timeline of tool calls extracted from transcript.
    pub tool_calls: Vec<ToolCall>,
    /// Unix-epoch ms of the assistant turn whose `tool_use` blocks are still
    /// awaiting the matching `user` response. Zero when the latest assistant
    /// turn has already been closed (no tools currently in flight).
    /// Used to animate the timeline bar for the running tool(s).
    pub pending_since_ms: u64,
    /// Unix-epoch ms of the most recent `user` line (prompt or tool_result)
    /// that has not yet been followed by an assistant response. Zero when
    /// the last transcript entry was an assistant turn. Used to render a
    /// live "Thinking" row while the model is generating its next reply.
    pub thinking_since_ms: u64,
    /// File access audit log: every file read/written/edited by the agent.
    pub file_accesses: Vec<FileAccess>,
    /// Config root directory for this session's agent (home-abbreviated, e.g. "~/.claude-work").
    /// For Claude Code: the active .claude* profile folder. For Codex: "~/.codex".
    /// For OpenCode: the data directory containing opencode.db.
    pub config_root: String,
}

impl AgentSession {
    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens
            + self.total_output_tokens
            + self.total_cache_read
            + self.total_cache_create
    }

    /// Tokens that represent new work (input + output), excluding cache hits.
    /// Used for rate calculation to avoid inflated numbers from cache_read.
    pub fn active_tokens(&self) -> u64 {
        self.total_input_tokens + self.total_output_tokens + self.total_cache_create
    }

    pub fn elapsed(&self) -> Duration {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Duration::from_millis(now.saturating_sub(self.started_at))
    }

    pub fn elapsed_display(&self) -> String {
        let secs = self.elapsed().as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SessionFile {
    pub pid: u32,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub cwd: String,
    #[serde(rename = "startedAt")]
    pub started_at: u64,
}

impl SessionFile {
    /// Truncate string fields to sane limits after deserialization.
    pub fn sanitize(&mut self) {
        truncate_string(&mut self.session_id, 256);
        truncate_string(&mut self.cwd, 4096);
    }
}

/// Truncate a string at a char boundary to avoid panics on multi-byte UTF-8.
fn truncate_string(s: &mut String, max_bytes: usize) {
    if s.len() > max_bytes {
        // Find the last char boundary at or before max_bytes
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(input: u64, output: u64, cache_read: u64, cache_create: u64) -> AgentSession {
        AgentSession {
            agent_cli: "claude",
            launch_surface: LaunchSurface::Cli,
            pid: 0,
            session_id: String::new(),
            cwd: String::new(),
            project_name: String::new(),
            started_at: 0,
            status: SessionStatus::Waiting,
            model: String::new(),
            effort: String::new(),
            context_percent: 0.0,
            total_input_tokens: input,
            total_output_tokens: output,
            total_cache_read: cache_read,
            total_cache_create: cache_create,
            model_usage: Vec::new(),
            turn_count: 0,
            current_tasks: Vec::new(),
            mem_mb: 0,
            version: String::new(),
            git_branch: String::new(),
            git_added: 0,
            git_modified: 0,
            token_history: Vec::new(),
            context_history: Vec::new(),
            compaction_count: 0,
            context_window: 0,
            subagent_counts: Default::default(),
            subagents: Vec::new(),
            mem_file_count: 0,
            mem_line_count: 0,
            children: Vec::new(),
            initial_prompt: String::new(),
            first_assistant_text: String::new(),
            chat_messages: Vec::new(),
            tool_calls: Vec::new(),
            pending_since_ms: 0,
            thinking_since_ms: 0,
            file_accesses: Vec::new(),
            config_root: String::new(),
        }
    }

    #[test]
    fn test_total_tokens() {
        let session = make_session(100, 50, 200, 30);
        assert_eq!(session.total_tokens(), 380); // 100 + 50 + 200 + 30
    }

    #[test]
    fn test_active_tokens() {
        let session = make_session(100, 50, 200, 30);
        assert_eq!(session.active_tokens(), 180); // 100 + 50 + 30, excludes cache_read
    }
}
