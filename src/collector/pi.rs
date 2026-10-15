use super::claude::{file_identity, read_records};
use super::process::{self, ProcInfo};
use crate::model::{
    token_total, AgentCounts, AgentSession, ChatMessage, ChatRole, ChildProcess, FileAccess,
    FileOp, LaunchSurface, ModelUsage, SessionStatus, SubAgent, ToolCall, MAX_CHAT_MESSAGES,
    MAX_FILE_ACCESSES,
};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(all(
    not(target_os = "linux"),
    not(target_vendor = "apple"),
    not(target_os = "windows")
))]
use std::process::Command;

#[derive(Debug, Default)]
struct ProcessOpenPaths {
    cwd: Option<PathBuf>,
    paths: Vec<PathBuf>,
    ttys: Vec<String>,
}

#[derive(Default)]
struct SubagentCacheEntry {
    offset: u64,
    file_identity: (u64, u64),
    totals: [u64; 4],
    model_usage: Vec<ModelUsage>,
    name: String,
    exited: bool,
}

#[derive(Clone, Copy)]
pub struct PiHarnessSpec {
    pub agent_cli: &'static str,
    pub default_dot_dir: &'static str,
    pub env_agent_dir: &'static str,
    pub env_session_dir: &'static str,
}

pub const OMP_SPEC: PiHarnessSpec = PiHarnessSpec {
    agent_cli: "omp",
    default_dot_dir: ".omp",
    env_agent_dir: "OMP_CODING_AGENT_DIR",
    env_session_dir: "OMP_CODING_AGENT_SESSION_DIR",
};

pub const PI_SPEC: PiHarnessSpec = PiHarnessSpec {
    agent_cli: "pi",
    default_dot_dir: ".pi",
    env_agent_dir: "PI_CODING_AGENT_DIR",
    env_session_dir: "PI_CODING_AGENT_SESSION_DIR",
};

pub struct PiCollector {
    spec: PiHarnessSpec,
    agent_dirs: Vec<PathBuf>,
    transcript_cache: HashMap<PathBuf, TranscriptState>,
    subagent_cache: HashMap<PathBuf, HashMap<PathBuf, SubagentCacheEntry>>,
    cached_git_branch: HashMap<String, String>,
    cached_versions: HashMap<PathBuf, String>,
}

struct TranscriptState {
    session_id: String,
    cwd: String,
    started_at: u64,
    title: String,
    model: String,
    effort: String,
    total_input: u64,
    total_output: u64,
    total_cache_read: u64,
    total_cache_create: u64,
    model_usage: Vec<ModelUsage>,
    last_context_tokens: u64,
    max_context_tokens: u64,
    context_history: Vec<u64>,
    token_history: Vec<u64>,
    compaction_count: u32,
    turn_count: u32,
    current_task: String,
    pending_tool_ids: HashSet<String>,
    pending_since_ms: u64,
    thinking_since_ms: u64,
    initial_prompt: String,
    first_assistant_text: String,
    chat_messages: Vec<ChatMessage>,
    tool_calls: Vec<ToolCall>,
    file_accesses: Vec<FileAccess>,
    new_offset: u64,
    file_identity: (u64, u64),
}

impl Default for TranscriptState {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            cwd: String::new(),
            started_at: 0,
            title: String::new(),
            model: "-".to_string(),
            effort: String::new(),
            total_input: 0,
            total_output: 0,
            total_cache_read: 0,
            total_cache_create: 0,
            model_usage: Vec::new(),
            last_context_tokens: 0,
            max_context_tokens: 0,
            context_history: Vec::new(),
            token_history: Vec::new(),
            compaction_count: 0,
            turn_count: 0,
            current_task: String::new(),
            pending_tool_ids: HashSet::new(),
            pending_since_ms: 0,
            thinking_since_ms: 0,
            initial_prompt: String::new(),
            first_assistant_text: String::new(),
            chat_messages: Vec::new(),
            tool_calls: Vec::new(),
            file_accesses: Vec::new(),
            new_offset: 0,
            file_identity: (0, 0),
        }
    }
}

impl PiCollector {
    pub fn new(spec: PiHarnessSpec) -> Self {
        Self {
            spec,
            agent_dirs: Vec::new(),
            transcript_cache: HashMap::new(),
            subagent_cache: HashMap::new(),
            cached_git_branch: HashMap::new(),
            cached_versions: HashMap::new(),
        }
    }

    pub fn new_omp() -> Self {
        Self::new(OMP_SPEC)
    }

    pub fn new_pi() -> Self {
        Self::new(PI_SPEC)
    }

    fn refresh_agent_dirs(&mut self) {
        let mut dirs = BTreeSet::new();
        if let Some(home) = dirs::home_dir() {
            dirs.insert(home.join(self.spec.default_dot_dir).join("agent"));
        }
        if let Ok(env_dir) = std::env::var(self.spec.env_agent_dir) {
            let p = PathBuf::from(env_dir);
            if p.is_dir() {
                dirs.insert(p);
            }
        }
        if let Ok(session_dir) = std::env::var(self.spec.env_session_dir) {
            let p = PathBuf::from(session_dir);
            if let Some(parent) = p.parent() {
                if parent.is_dir() {
                    dirs.insert(parent.to_path_buf());
                }
            }
        }
        self.agent_dirs = dirs.into_iter().collect();
    }

    fn find_pids(&self, process_info: &HashMap<u32, ProcInfo>, self_pid: u32) -> Vec<u32> {
        let mut pids = Vec::new();
        for (&pid, info) in process_info {
            if pid == self_pid || process::is_descendant_of(pid, self_pid, process_info) {
                continue;
            }
            let cmd = info.command.as_str();
            if !cmd_matches_harness(cmd, self.spec.agent_cli) {
                continue;
            }
            if is_background_worker_command(cmd, self.spec.agent_cli) {
                continue;
            }
            pids.push(pid);
        }

        // Drop wrapper ancestors when a child process is also in the candidate set
        // (e.g. `/bin/sh ~/.pi/agent/bin/pi` spawning `node .../pi`).
        let candidates = pids.clone();
        pids.retain(|&pid| {
            !candidates
                .iter()
                .any(|&other| other != pid && process::is_descendant_of(other, pid, process_info))
        });
        pids.sort_unstable();
        pids
    }

    fn collect_sessions_with_observer(
        &mut self,
        shared: &super::SharedProcessData,
        completed: &mut dyn FnMut(&mut AgentSession),
    ) -> Vec<AgentSession> {
        if shared.slow_tick || self.agent_dirs.is_empty() {
            self.refresh_agent_dirs();
            self.cached_git_branch.clear();
        }

        let self_pid = std::process::id();
        let pids = self.find_pids(&shared.process_info, self_pid);
        if pids.is_empty() {
            self.transcript_cache.clear();
            self.subagent_cache.clear();
            return Vec::new();
        }

        let open_paths = map_pid_to_open_paths(&pids);
        self.merge_agent_dirs_from_open_paths(&open_paths);

        let mut sessions = Vec::new();
        let mut claimed_files = HashSet::new();
        let mut active_files = HashSet::new();

        for &pid in &pids {
            let Some(proc_info) = shared.process_info.get(&pid) else {
                continue;
            };
            let proc_open = open_paths.get(&pid);
            let (jsonl_path, agent_dir, synthetic_cwd) = match self.resolve_session_file_for_pid(
                pid,
                &proc_info.command,
                proc_open,
                &claimed_files,
            ) {
                Some((path, dir)) => (Some(path), dir, None),
                None => {
                    let fallback_cwd = proc_open
                        .and_then(|o| o.cwd.clone())
                        .or_else(|| get_process_cwd(pid));
                    let Some(cwd) = fallback_cwd else {
                        continue;
                    };
                    let Some(dir) = self.agent_dirs.first().cloned() else {
                        continue;
                    };
                    (None, dir, Some(cwd))
                }
            };

            if let Some(path) = &jsonl_path {
                claimed_files.insert(path.clone());
                active_files.insert(path.clone());
            }

            if let Some(mut session) = self.load_session(
                pid,
                jsonl_path.as_deref(),
                &agent_dir,
                synthetic_cwd
                    .as_deref()
                    .or_else(|| proc_open.and_then(|o| o.cwd.as_deref())),
                proc_open,
                &shared.process_info,
                &shared.children_map,
                &shared.ports,
            ) {
                completed(&mut session);
                sessions.push(session);
            }
        }

        self.transcript_cache
            .retain(|path, _| active_files.contains(path));
        self.subagent_cache.retain(|dir, _| {
            let parent_jsonl = dir.with_extension("jsonl");
            active_files.contains(&parent_jsonl)
        });

        sessions.sort_by_key(|s| std::cmp::Reverse(s.started_at));
        sessions
    }

    fn merge_agent_dirs_from_open_paths(&mut self, open_paths: &HashMap<u32, ProcessOpenPaths>) {
        let mut seen: BTreeSet<PathBuf> = self.agent_dirs.iter().cloned().collect();
        for info in open_paths.values() {
            for path in &info.paths {
                for ancestor in path.ancestors() {
                    if ancestor.file_name().and_then(|n| n.to_str()) == Some("sessions") {
                        if let Some(agent_dir) = ancestor.parent() {
                            if agent_dir.is_dir() {
                                seen.insert(agent_dir.to_path_buf());
                            }
                        }
                    }
                }
            }
        }
        self.agent_dirs = seen.into_iter().collect();
    }

    fn resolve_session_file_for_pid(
        &self,
        pid: u32,
        command: &str,
        open_paths: Option<&ProcessOpenPaths>,
        claimed_files: &HashSet<PathBuf>,
    ) -> Option<(PathBuf, PathBuf)> {
        // 1. Direct open file descriptor to a root session `.jsonl` under `sessions/<project>/<file>.jsonl`
        if let Some(open) = open_paths {
            for path in &open.paths {
                if let Some((session_path, agent_dir)) =
                    classify_open_session_path(path, &self.agent_dirs)
                {
                    if !claimed_files.contains(&session_path)
                        && session_path.is_file()
                        && !is_symlink(&session_path)
                    {
                        return Some((session_path, agent_dir));
                    }
                }
            }

            // 2. Open lock file in `run/session-owners/<sid>.lock` or `.<file>.jsonl.owner.lock`
            for path in &open.paths {
                if let Some((session_path, agent_dir)) =
                    self.resolve_lock_file_path(path, open.cwd.as_deref())
                {
                    if !claimed_files.contains(&session_path)
                        && session_path.is_file()
                        && !is_symlink(&session_path)
                    {
                        return Some((session_path, agent_dir));
                    }
                }
            }
        }

        // 3. Explicit CLI flags: `--session <path|id>` or `--resume <path|id>`
        if let Some(arg) = extract_cli_session_arg(command) {
            if let Some(resolved) = self.resolve_session_arg(
                &arg,
                open_paths.and_then(|o| o.cwd.as_deref()),
                claimed_files,
            ) {
                return Some(resolved);
            }
        }

        // 4. Terminal session file in `<agent_dir>/terminal-sessions/<tty>`
        if let Some(open) = open_paths {
            for tty in &open.ttys {
                for agent_dir in &self.agent_dirs {
                    let ts_file = agent_dir.join("terminal-sessions").join(tty);
                    if let Some(candidate) = read_terminal_session_file(&ts_file) {
                        if !claimed_files.contains(&candidate)
                            && candidate.is_file()
                            && !is_symlink(&candidate)
                        {
                            return Some((candidate, agent_dir.clone()));
                        }
                    }
                }
            }
        }

        // 5. Fallback: most recent `.jsonl` in the encoded cwd directory under `<agent_dir>/sessions/`
        let fallback_cwd = open_paths
            .and_then(|o| o.cwd.clone())
            .or_else(|| get_process_cwd(pid));
        if let Some(cwd) = fallback_cwd.as_deref() {
            for agent_dir in &self.agent_dirs {
                let sessions_root = agent_dir.join("sessions");
                for dir in candidate_cwd_session_dirs(&sessions_root, cwd) {
                    if let Some(best) = most_recent_jsonl_in_dir(&dir, claimed_files) {
                        return Some((best, agent_dir.clone()));
                    }
                }
            }
        }

        None
    }

    fn resolve_lock_file_path(
        &self,
        path: &Path,
        cwd: Option<&Path>,
    ) -> Option<(PathBuf, PathBuf)> {
        let name = path.file_name()?.to_str()?;
        // Case A: `.2026-..._<sid>.jsonl.owner.lock` next to the session file
        if let Some(stripped) = name
            .strip_prefix('.')
            .and_then(|s| s.strip_suffix(".owner.lock"))
        {
            if stripped.ends_with(".jsonl") {
                let session_path = path.with_file_name(stripped);
                let agent_dir = infer_agent_dir_from_session_path(&session_path, &self.agent_dirs);
                return Some((session_path, agent_dir));
            }
        }
        // Case B: `<sid>.lock` under `run/session-owners/`
        if let Some(sid) = name.strip_suffix(".lock") {
            if path.parent()?.file_name()?.to_str()? == "session-owners" {
                return self.find_session_by_id(sid, cwd);
            }
        }
        None
    }

    fn resolve_session_arg(
        &self,
        arg: &str,
        cwd: Option<&Path>,
        claimed_files: &HashSet<PathBuf>,
    ) -> Option<(PathBuf, PathBuf)> {
        let path = PathBuf::from(arg);
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && path.is_file()
            && !is_symlink(&path)
            && !claimed_files.contains(&path)
        {
            let agent_dir = infer_agent_dir_from_session_path(&path, &self.agent_dirs);
            return Some((path, agent_dir));
        }
        self.find_session_by_id(arg, cwd)
            .filter(|(p, _)| !claimed_files.contains(p))
    }

    fn find_session_by_id(&self, sid: &str, cwd: Option<&Path>) -> Option<(PathBuf, PathBuf)> {
        if sid.is_empty() {
            return None;
        }
        let suffix = format!("_{sid}.jsonl");
        for agent_dir in &self.agent_dirs {
            let sessions_root = agent_dir.join("sessions");
            if let Some(cwd) = cwd {
                for dir in candidate_cwd_session_dirs(&sessions_root, cwd) {
                    if let Some(found) = find_file_with_suffix_or_name(&dir, sid, &suffix) {
                        return Some((found, agent_dir.clone()));
                    }
                }
            }
            let Ok(entries) = fs::read_dir(&sessions_root) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(true) {
                    continue;
                }
                let dir = entry.path();
                if !dir.is_dir() {
                    continue;
                }
                if let Some(found) = find_file_with_suffix_or_name(&dir, sid, &suffix) {
                    return Some((found, agent_dir.clone()));
                }
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn load_session(
        &mut self,
        pid: u32,
        jsonl_path: Option<&Path>,
        agent_dir: &Path,
        proc_cwd: Option<&Path>,
        proc_open: Option<&ProcessOpenPaths>,
        process_info: &HashMap<u32, ProcInfo>,
        children_map: &HashMap<u32, Vec<u32>>,
        ports: &HashMap<u32, Vec<u16>>,
    ) -> Option<AgentSession> {
        let (subagents, subagent_counts, sub_totals, sub_models) = if let Some(jp) = jsonl_path {
            let subagent_dir = jp.with_extension("");
            self.collect_subagents(&subagent_dir)
        } else {
            (Vec::new(), AgentCounts::default(), [0; 4], Vec::new())
        };
        let [sub_in, sub_out, sub_cr, sub_cw] = sub_totals;

        let default_state = TranscriptState::default();
        let state = if let Some(jp) = jsonl_path {
            let identity = file_identity(jp);
            let file_len = fs::metadata(jp).map(|m| m.len()).ok()?;
            let st = self.transcript_cache.entry(jp.to_path_buf()).or_default();
            if st.file_identity != identity || file_len < st.new_offset {
                *st = TranscriptState::default();
                st.file_identity = identity;
            }
            update_transcript_state(jp, st);
            &*st
        } else {
            &default_state
        };

        let cwd = if !state.cwd.is_empty() {
            state.cwd.clone()
        } else if let Some(c) = proc_cwd {
            c.to_string_lossy().into_owned()
        } else {
            String::new()
        };

        let session_id = if !state.session_id.is_empty() {
            state.session_id.clone()
        } else if let Some(jp) = jsonl_path {
            session_id_from_filename(jp)
        } else {
            format!("pid-{pid}")
        };

        if session_id.is_empty() {
            return None;
        }

        let started_at = if state.started_at > 0 {
            state.started_at
        } else if let Some(jp) = jsonl_path {
            fs::metadata(jp)
                .ok()
                .and_then(|m| m.created().or_else(|_| m.modified()).ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        } else {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        };
        let project_name = process::last_path_segment(&cwd)
            .filter(|s| !s.is_empty())
            .unwrap_or("?")
            .to_string();
        let mut model_usage = state.model_usage.clone();
        ModelUsage::merge(&mut model_usage, &sub_models);

        let has_working_subagent = subagent_counts.working > 0;
        let pending_tool = !state.current_task.is_empty();
        let model_generating = state.thinking_since_ms > 0;
        let proc = process_info.get(&pid);
        let cpu_active = proc.is_some_and(|p| p.cpu_pct > 2.0);
        let descendant_active =
            process::has_active_descendant(pid, children_map, process_info, 5.0);

        let status = if pending_tool {
            SessionStatus::Executing
        } else if model_generating {
            SessionStatus::Thinking
        } else if has_working_subagent {
            SessionStatus::Background
        } else if descendant_active {
            SessionStatus::Executing
        } else if cpu_active {
            SessionStatus::Thinking
        } else {
            SessionStatus::Waiting
        };

        let current_tasks = if !state.current_task.is_empty() {
            vec![state.current_task.clone()]
        } else if matches!(status, SessionStatus::Background) {
            if let Some(sa) = subagents.iter().rev().find(|a| a.status == "working") {
                vec![format!("background agent {}", sa.name)]
            } else {
                vec!["background work".to_string()]
            }
        } else if matches!(status, SessionStatus::Waiting) {
            vec!["waiting for input".to_string()]
        } else {
            vec!["thinking...".to_string()]
        };

        let context_window =
            super::context_window_for_model(&state.model, "", state.max_context_tokens);
        let context_percent = if context_window > 0 {
            (state.last_context_tokens as f64 / context_window as f64) * 100.0
        } else {
            0.0
        };

        let mut children = Vec::new();
        let mut stack: Vec<u32> = children_map.get(&pid).cloned().unwrap_or_default();
        let mut visited = HashSet::new();
        let mut tree_rss_kb = proc.map(|p| p.rss_kb).unwrap_or(0);
        while let Some(cpid) = stack.pop() {
            if !visited.insert(cpid) {
                continue;
            }
            if let Some(cproc) = process_info.get(&cpid) {
                tree_rss_kb = tree_rss_kb.saturating_add(cproc.rss_kb);
                let port = ports.get(&cpid).and_then(|v| v.first().copied());
                children.push(ChildProcess {
                    pid: cpid,
                    command: cproc.command.clone(),
                    mem_kb: cproc.rss_kb,
                    port,
                });
            }
            if let Some(grandchildren) = children_map.get(&cpid) {
                stack.extend(grandchildren);
            }
        }

        let initial_prompt = if !state.title.is_empty() {
            state.title.clone()
        } else {
            state.initial_prompt.clone()
        };

        let config_root = agent_dir
            .parent()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n == self.spec.default_dot_dir)
            })
            .unwrap_or(agent_dir);
        let total_input_tokens = state.total_input + sub_in;
        let total_output_tokens = state.total_output + sub_out;
        let total_cache_read = state.total_cache_read + sub_cr;
        let total_cache_create = state.total_cache_create + sub_cw;
        let model = state.model.clone();
        let effort = state.effort.clone();
        let turn_count = state.turn_count;
        let token_history = state.token_history.clone();
        let context_history = state.context_history.clone();
        let compaction_count = state.compaction_count;
        let first_assistant_text = state.first_assistant_text.clone();
        let chat_messages = state.chat_messages.clone();
        let tool_calls = state.tool_calls.clone();
        let pending_since_ms = state.pending_since_ms;
        let thinking_since_ms = state.thinking_since_ms;
        let file_accesses = state.file_accesses.clone();

        let git_branch = if !cwd.is_empty() {
            self.cached_git_branch
                .entry(cwd.clone())
                .or_insert_with(|| resolve_git_branch(&cwd))
                .clone()
        } else {
            String::new()
        };

        let version = resolve_harness_version(
            self.spec.agent_cli,
            agent_dir,
            proc_open,
            &mut self.cached_versions,
        );

        Some(AgentSession {
            agent_cli: self.spec.agent_cli,
            launch_surface: LaunchSurface::Cli,
            pid,
            session_id,
            cwd,
            project_name,
            started_at,
            status,
            model,
            effort,
            context_percent,
            total_input_tokens,
            total_output_tokens,
            total_cache_read,
            total_cache_create,
            model_usage: ModelUsage::sorted(&model_usage),
            turn_count,
            current_tasks,
            mem_mb: tree_rss_kb / 1024,
            version,
            git_branch,
            git_added: 0,
            git_modified: 0,
            token_history,
            context_history,
            compaction_count,
            context_window,
            subagent_counts,
            subagents,
            pending_workflows: 0,
            pending_bg_agents: subagent_counts.working,
            mem_file_count: 0,
            mem_line_count: 0,
            children,
            initial_prompt,
            first_assistant_text,
            chat_messages,
            tool_calls,
            pending_since_ms,
            thinking_since_ms,
            file_accesses,
            config_root: super::abbrev_path(config_root),
        })
    }

    fn collect_subagents(
        &mut self,
        subagent_dir: &Path,
    ) -> (Vec<SubAgent>, AgentCounts, [u64; 4], Vec<ModelUsage>) {
        if !fs::symlink_metadata(subagent_dir).is_ok_and(|m| m.is_dir()) {
            return (Vec::new(), AgentCounts::default(), [0; 4], Vec::new());
        }
        let mut paths = Vec::new();
        super::activity::files_under(subagent_dir, &mut paths);
        paths.retain(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                && !p
                    .components()
                    .any(|c| c.as_os_str() == "local" || c.as_os_str() == "artifacts")
        });
        paths.sort();

        let cache = self
            .subagent_cache
            .entry(subagent_dir.to_path_buf())
            .or_default();
        let active_set: HashSet<PathBuf> = paths.iter().cloned().collect();
        cache.retain(|k, _| active_set.contains(k));

        let mut subagents = Vec::new();
        let mut counts = AgentCounts::default();
        let mut totals = [0u64; 4];
        let mut model_usage = Vec::new();

        for path in paths {
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let entry = cache.entry(path.clone()).or_default();
            let id = file_identity(&path);
            if id != entry.file_identity || meta.len() < entry.offset {
                *entry = SubagentCacheEntry::default();
                entry.file_identity = id;
            }
            if entry.name.is_empty() {
                entry.name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("subagent")
                    .trim_start_matches("__")
                    .to_string();
            }

            let read = read_records(&path, entry.offset, |val| {
                match val.get("type").and_then(Value::as_str) {
                    Some("session_init") => {
                        if let Some(agent) = val.get("agent").and_then(Value::as_str) {
                            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(agent);
                            if stem != "session" {
                                entry.name = stem.to_string();
                            } else {
                                entry.name = agent.to_string();
                            }
                        }
                    }
                    Some("custom")
                        if val.get("customType").and_then(Value::as_str)
                            == Some("session_exit") =>
                    {
                        entry.exited = true;
                    }
                    Some("message") => {
                        let msg = &val["message"];
                        if msg.get("role").and_then(Value::as_str) == Some("assistant") {
                            if let Some(u) = msg.get("usage") {
                                let [inp, out, cr, cw] = extract_pi_usage(u);
                                entry.totals[0] += inp;
                                entry.totals[1] += out;
                                entry.totals[2] += cr;
                                entry.totals[3] += cw;
                                let m = format_pi_model(
                                    msg.get("provider").and_then(Value::as_str),
                                    msg.get("model").and_then(Value::as_str),
                                );
                                ModelUsage::add(
                                    &mut entry.model_usage,
                                    &m,
                                    token_total(inp, out, cw),
                                );
                            }
                        }
                    }
                    Some("usage") | Some("model_usage") => {
                        if let Some(u) = val.get("usage") {
                            let [inp, out, cr, cw] = extract_pi_usage(u);
                            entry.totals[0] += inp;
                            entry.totals[1] += out;
                            entry.totals[2] += cr;
                            entry.totals[3] += cw;
                            let m = format_pi_model(
                                val.get("provider").and_then(Value::as_str),
                                val.get("model").and_then(Value::as_str),
                            );
                            ModelUsage::add(&mut entry.model_usage, &m, token_total(inp, out, cw));
                        }
                    }
                    _ => {}
                }
            });
            entry.offset = read.offset;

            for (acc, &v) in totals.iter_mut().zip(&entry.totals) {
                *acc += v;
            }
            ModelUsage::merge(&mut model_usage, &entry.model_usage);

            let is_working = if entry.exited {
                false
            } else {
                meta.modified()
                    .ok()
                    .and_then(|m| m.elapsed().ok())
                    .is_some_and(|age| age.as_secs() < 45)
            };
            if is_working {
                counts.working += 1;
            }

            let rel_key = path
                .strip_prefix(subagent_dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            subagents.push(SubAgent {
                activity_key: rel_key,
                name: truncate_chars(&entry.name, 30),
                status: if is_working {
                    "working".to_string()
                } else {
                    "done".to_string()
                },
                tokens: token_total(entry.totals[0], entry.totals[1], entry.totals[3]),
                workflow: None,
            });
        }

        (subagents, counts, totals, model_usage)
    }
}

impl super::StartupCollector for PiCollector {
    fn collect_startup(
        &mut self,
        shared: &super::SharedProcessData,
        completed: &mut dyn FnMut(&mut AgentSession),
    ) -> Vec<AgentSession> {
        self.collect_sessions_with_observer(shared, completed)
    }
}

impl super::AgentCollector for PiCollector {
    fn collect(&mut self, shared: &super::SharedProcessData) -> Vec<AgentSession> {
        self.collect_sessions_with_observer(shared, &mut |_| {})
    }
}

pub(crate) fn cmd_matches_harness(cmd: &str, name: &str) -> bool {
    if cmd.contains("grep") {
        return false;
    }
    if process::cmd_has_binary(cmd, name) {
        return true;
    }
    if name == "pi" {
        return cmd.split_whitespace().any(is_pi_package_entry);
    }
    false
}

/// A pi entry point reached through the npm package rather than a `pi` binary
/// (e.g. `node .../@earendil-works/pi-coding-agent/dist/bundle/cli.js`).
fn is_pi_package_entry(tok: &str) -> bool {
    let normalized = tok.replace('\\', "/");
    normalized.contains("/@earendil-works/pi-coding-agent/")
        || normalized.ends_with("/node_modules/.bin/pi")
}

fn is_background_worker_command(cmd: &str, name: &str) -> bool {
    if name == "omp" && cmd.contains("__omp_worker_") {
        return true;
    }
    // Skip non-interactive subcommands for pi (`pi list`, `pi config`, `pi update`, `pi mcp`, `pi auth`),
    // whether invoked through the launcher or its node child running the package entry.
    if name == "pi" {
        let tokens: Vec<&str> = cmd.split_whitespace().collect();
        for (idx, tok) in tokens.iter().enumerate().take(2) {
            if tok.ends_with("/pi") || *tok == "pi" || is_pi_package_entry(tok) {
                if let Some(&sub) = tokens.get(idx + 1) {
                    if matches!(
                        sub,
                        "install"
                            | "remove"
                            | "uninstall"
                            | "update"
                            | "list"
                            | "config"
                            | "auth"
                            | "mcp"
                    ) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn update_transcript_state(path: &Path, state: &mut TranscriptState) {
    let read = read_records(path, state.new_offset, |val| {
        let entry_ts_ms = parse_entry_timestamp_ms(val);

        match val.get("type").and_then(Value::as_str) {
            Some("session") => {
                if let Some(id) = val.get("id").and_then(Value::as_str) {
                    state.session_id = super::sanitize_terminal_text(id);
                }
                if let Some(cwd) = val.get("cwd").and_then(Value::as_str) {
                    state.cwd = super::sanitize_terminal_text(cwd);
                }
                if entry_ts_ms > 0 {
                    state.started_at = entry_ts_ms;
                }
                if let Some(title) = val.get("title").and_then(Value::as_str) {
                    let clean = clean_text(title, 80);
                    if !clean.is_empty() {
                        state.title = clean;
                    }
                }
            }
            Some("title") | Some("title_change") => {
                if let Some(title) = val.get("title").and_then(Value::as_str) {
                    let clean = clean_text(title, 80);
                    if !clean.is_empty() {
                        state.title = clean;
                    }
                }
            }
            Some("model_change") => {
                // Only update primary session model when `role` is absent or `"default"`
                let role = val.get("role").and_then(Value::as_str).unwrap_or("default");
                if role == "default" {
                    let m = if let Some(model) = val.get("model").and_then(Value::as_str) {
                        format_pi_model(val.get("provider").and_then(Value::as_str), Some(model))
                    } else if let Some(model_id) = val.get("modelId").and_then(Value::as_str) {
                        format_pi_model(val.get("provider").and_then(Value::as_str), Some(model_id))
                    } else {
                        String::new()
                    };
                    if !m.is_empty() && m != "-" {
                        state.model = m;
                    }
                }
            }
            Some("thinking_level_change") => {
                if let Some(level) = val.get("thinkingLevel").and_then(Value::as_str) {
                    let clean = super::sanitize_terminal_text(level.trim());
                    if !clean.is_empty() {
                        state.effort = clean;
                    }
                }
            }
            Some("compaction") => {
                state.compaction_count += 1;
                if let Some(after) = val.get("tokensAfter").and_then(Value::as_u64) {
                    if after > 0 {
                        state.last_context_tokens = after;
                        if state.context_history.len() < 10_000 {
                            state.context_history.push(after);
                        }
                    }
                }
                if let Some(u) = val.get("usage") {
                    let [inp, out, cr, cw] = extract_pi_usage(u);
                    state.total_input += inp;
                    state.total_output += out;
                    state.total_cache_read += cr;
                    state.total_cache_create += cw;
                    let m = state.model.clone();
                    ModelUsage::add(&mut state.model_usage, &m, token_total(inp, out, cw));
                }
            }
            Some("branch_summary") => {
                state.thinking_since_ms = 0;
                state.pending_since_ms = 0;
                state.current_task.clear();
                state.pending_tool_ids.clear();
            }
            Some("usage") | Some("model_usage") => {
                if let Some(u) = val.get("usage") {
                    let [inp, out, cr, cw] = extract_pi_usage(u);
                    state.total_input += inp;
                    state.total_output += out;
                    state.total_cache_read += cr;
                    state.total_cache_create += cw;
                    let m = format_pi_model(
                        val.get("provider").and_then(Value::as_str),
                        val.get("model").and_then(Value::as_str),
                    );
                    ModelUsage::add(&mut state.model_usage, &m, token_total(inp, out, cw));
                }
            }
            Some("custom") => {
                if val.get("customType").and_then(Value::as_str) == Some("tool_execution_start") {
                    if let Some(data) = val.get("data") {
                        let tool_name = data
                            .get("toolName")
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        let arg = data
                            .get("intent")
                            .and_then(Value::as_str)
                            .map(|s| truncate_chars(&super::sanitize_terminal_text(s), 40))
                            .unwrap_or_else(|| extract_pi_tool_arg(tool_name, data.get("args")));
                        state.current_task = if arg.is_empty() {
                            canonical_tool_name(tool_name).to_string()
                        } else {
                            format!("{} {}", canonical_tool_name(tool_name), arg)
                        };
                        if let Some(id) = data.get("toolCallId").and_then(Value::as_str) {
                            state.pending_tool_ids.insert(id.to_string());
                        }
                        if state.pending_since_ms == 0 && entry_ts_ms > 0 {
                            state.pending_since_ms = entry_ts_ms;
                        }
                    }
                }
            }
            Some("message") => {
                let Some(msg) = val.get("message") else {
                    return;
                };
                match msg.get("role").and_then(Value::as_str) {
                    Some("user") => {
                        let prompt_text = extract_message_text(msg, 200);
                        if state.initial_prompt.is_empty() && !prompt_text.is_empty() {
                            state.initial_prompt = truncate_chars(&prompt_text, 80);
                        }
                        if !prompt_text.is_empty() {
                            push_chat_message(
                                &mut state.chat_messages,
                                ChatRole::User,
                                prompt_text,
                            );
                        }
                        state.pending_tool_ids.clear();
                        state.current_task.clear();
                        state.pending_since_ms = 0;
                        if entry_ts_ms > 0 {
                            state.thinking_since_ms = entry_ts_ms;
                        }
                    }
                    Some("assistant") => {
                        state.turn_count += 1;
                        state.thinking_since_ms = 0;
                        state.current_task.clear();
                        state.pending_tool_ids.clear();
                        state.pending_since_ms = 0;

                        let msg_model = format_pi_model(
                            msg.get("provider").and_then(Value::as_str),
                            msg.get("model").and_then(Value::as_str),
                        );
                        if msg_model != "-" {
                            state.model = msg_model.clone();
                        }

                        if let Some(effort) = msg
                            .get("requestControls")
                            .and_then(|rc| rc.get("effort"))
                            .and_then(|ef| ef.get("topLevel"))
                            .and_then(Value::as_str)
                        {
                            let clean = super::sanitize_terminal_text(effort.trim());
                            if !clean.is_empty() {
                                state.effort = clean;
                            }
                        }

                        if let Some(usage) = msg.get("usage") {
                            let [inp, out, cr, cw] = extract_pi_usage(usage);
                            state.total_input += inp;
                            state.total_output += out;
                            state.total_cache_read += cr;
                            state.total_cache_create += cw;
                            let model_for_usage = if msg_model != "-" {
                                msg_model
                            } else {
                                state.model.clone()
                            };
                            let turn_tokens = token_total(inp, out, cw);
                            ModelUsage::add(&mut state.model_usage, &model_for_usage, turn_tokens);
                            if state.token_history.len() < 10_000 {
                                state.token_history.push(turn_tokens);
                            }

                            let snap_prompt = msg
                                .get("contextSnapshot")
                                .and_then(|cs| cs.get("promptTokens"))
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            let current_context = if snap_prompt > 0 {
                                snap_prompt
                            } else if cr == 0 && cw > 0 {
                                inp + cw
                            } else {
                                inp + cr
                            };
                            if current_context > 0 {
                                state.last_context_tokens = current_context;
                                if current_context > state.max_context_tokens {
                                    state.max_context_tokens = current_context;
                                }
                                if state.context_history.len() < 10_000 {
                                    state.context_history.push(current_context);
                                }
                            }
                        }

                        let assistant_text = extract_message_text(msg, 500);
                        if state.first_assistant_text.is_empty() && !assistant_text.is_empty() {
                            state.first_assistant_text = truncate_chars(&assistant_text, 200);
                        }
                        if !assistant_text.is_empty() {
                            push_chat_message(
                                &mut state.chat_messages,
                                ChatRole::Assistant,
                                assistant_text,
                            );
                        }

                        let mut has_tool_call = false;
                        if let Some(arr) = msg.get("content").and_then(Value::as_array) {
                            for item in arr {
                                if item.get("type").and_then(Value::as_str) != Some("toolCall") {
                                    continue;
                                }
                                has_tool_call = true;
                                let raw_name =
                                    item.get("name").and_then(Value::as_str).unwrap_or("?");
                                let display_name = canonical_tool_name(raw_name);
                                let args = item.get("arguments");
                                let arg = item
                                    .get("intent")
                                    .and_then(Value::as_str)
                                    .map(|s| truncate_chars(&super::sanitize_terminal_text(s), 40))
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or_else(|| extract_pi_tool_arg(raw_name, args));

                                state.current_task = if arg.is_empty() {
                                    display_name.to_string()
                                } else {
                                    format!("{display_name} {arg}")
                                };

                                if let Some(id) = item.get("id").and_then(Value::as_str) {
                                    state.pending_tool_ids.insert(id.to_string());
                                }

                                if state.tool_calls.len() < 500 {
                                    state.tool_calls.push(ToolCall {
                                        name: display_name.to_string(),
                                        arg: truncate_chars(&arg, 40),
                                        duration_ms: 0,
                                    });
                                }

                                record_file_access(
                                    &mut state.file_accesses,
                                    raw_name,
                                    args,
                                    state.turn_count,
                                );
                            }
                        }

                        if has_tool_call && entry_ts_ms > 0 {
                            state.pending_since_ms = entry_ts_ms;
                        }
                    }
                    Some("toolResult") => {
                        if let Some(id) = msg.get("toolCallId").and_then(Value::as_str) {
                            state.pending_tool_ids.remove(id);
                        }
                        if state.pending_tool_ids.is_empty() {
                            if entry_ts_ms > 0 && state.pending_since_ms > 0 {
                                let duration = entry_ts_ms.saturating_sub(state.pending_since_ms);
                                let pending: Vec<usize> = state
                                    .tool_calls
                                    .iter()
                                    .enumerate()
                                    .rev()
                                    .take_while(|(_, tc)| tc.duration_ms == 0)
                                    .map(|(i, _)| i)
                                    .collect();
                                if !pending.is_empty() {
                                    let per_call = (duration / pending.len() as u64).max(1);
                                    for idx in pending {
                                        state.tool_calls[idx].duration_ms = per_call;
                                    }
                                }
                            }
                            state.current_task.clear();
                            state.pending_since_ms = 0;
                            if entry_ts_ms > 0 {
                                state.thinking_since_ms = entry_ts_ms;
                            }
                        }
                        if let Some(details) = msg.get("details") {
                            if let Some(u) = details.get("usage") {
                                let [inp, out, cr, cw] = extract_pi_usage(u);
                                state.total_input += inp;
                                state.total_output += out;
                                state.total_cache_read += cr;
                                state.total_cache_create += cw;
                                let adv_model = details
                                    .get("advisorModel")
                                    .and_then(Value::as_str)
                                    .map(|m| format_pi_model(None, Some(m)))
                                    .filter(|s| !s.is_empty() && s != "-")
                                    .unwrap_or_else(|| state.model.clone());
                                ModelUsage::add(
                                    &mut state.model_usage,
                                    &adv_model,
                                    token_total(inp, out, cw),
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });

    if state.file_accesses.len() > MAX_FILE_ACCESSES {
        let excess = state.file_accesses.len() - MAX_FILE_ACCESSES;
        state.file_accesses.drain(..excess);
    }
    state.new_offset = read.offset;
}

fn extract_pi_usage(usage: &Value) -> [u64; 4] {
    let inp = usage
        .get("input")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let out = usage
        .get("output")
        .or_else(|| usage.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cr = usage
        .get("cacheRead")
        .or_else(|| usage.get("cache_read_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cw = usage
        .get("cacheWrite")
        .or_else(|| usage.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + usage
            .get("cacheWrite1h")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    [inp, out, cr, cw]
}

fn format_pi_model(_provider: Option<&str>, model: Option<&str>) -> String {
    let Some(raw) = model else {
        return "-".to_string();
    };
    let clean = super::sanitize_terminal_text(raw.trim());
    if clean.is_empty() {
        return "-".to_string();
    }
    // Strip leading `<provider>/` or `<provider>:` so the model column stays concise and consistent
    let stripped = clean
        .rsplit_once('/')
        .map(|(_, m)| m)
        .or_else(|| clean.rsplit_once(':').map(|(_, m)| m))
        .unwrap_or(&clean);
    if stripped.is_empty() {
        clean
    } else {
        stripped.to_string()
    }
}

fn canonical_tool_name(name: &str) -> &str {
    match name {
        "read" => "Read",
        "write" => "Write",
        "edit" | "ast_edit" => "Edit",
        "bash" | "eval" => "Bash",
        "grep" => "Grep",
        "glob" | "ls" => "Glob",
        "find" => "find",
        "task" | "advisor" => "Agent",
        "todo" => "update_plan",
        "wait" => "wait_agent",
        "web_search" | "fetch_content" => "WebSearch",
        other => other.trim(),
    }
    .trim()
}

fn extract_pi_tool_arg(tool_name: &str, args: Option<&Value>) -> String {
    let Some(args) = args else {
        return String::new();
    };
    if let Some(p) = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .and_then(Value::as_str)
    {
        return shorten_path(&super::sanitize_terminal_text(p));
    }
    if let Some(cmd) = args
        .get("command")
        .or_else(|| args.get("code"))
        .or_else(|| args.get("query"))
        .and_then(Value::as_str)
    {
        let first = cmd.lines().next().unwrap_or(cmd).trim();
        return super::redact_secrets(&truncate_chars(&super::sanitize_terminal_text(first), 40));
    }
    if let Some(pat) = args.get("pattern").and_then(Value::as_str) {
        return truncate_chars(&super::sanitize_terminal_text(pat), 40);
    }
    if tool_name == "edit" {
        if let Some(input) = args.get("input").and_then(Value::as_str) {
            for line in input.lines() {
                let t = line.trim();
                if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    let path_part = inner.split('#').next().unwrap_or(inner);
                    return shorten_path(&super::sanitize_terminal_text(path_part));
                }
            }
        }
    }
    String::new()
}

fn record_file_access(
    accesses: &mut Vec<FileAccess>,
    tool_name: &str,
    args: Option<&Value>,
    turn_index: u32,
) {
    let Some(args) = args else {
        return;
    };
    match tool_name {
        "read" | "Read" => {
            if let Some(p) = args
                .get("path")
                .or_else(|| args.get("file_path"))
                .and_then(Value::as_str)
            {
                let base = p.split(':').next().unwrap_or(p);
                if !base.is_empty() && !base.contains("://") {
                    accesses.push(FileAccess {
                        path: super::sanitize_terminal_text(base),
                        operation: FileOp::Read,
                        turn_index,
                    });
                }
            }
        }
        "write" | "Write" => {
            if let Some(p) = args
                .get("path")
                .or_else(|| args.get("file_path"))
                .and_then(Value::as_str)
            {
                if !p.is_empty() && !p.contains("://") {
                    accesses.push(FileAccess {
                        path: super::sanitize_terminal_text(p),
                        operation: FileOp::Write,
                        turn_index,
                    });
                }
            }
        }
        "edit" | "Edit" => {
            if let Some(p) = args
                .get("path")
                .or_else(|| args.get("file_path"))
                .and_then(Value::as_str)
            {
                if !p.is_empty() {
                    accesses.push(FileAccess {
                        path: super::sanitize_terminal_text(p),
                        operation: FileOp::Edit,
                        turn_index,
                    });
                }
            } else if let Some(input) = args.get("input").and_then(Value::as_str) {
                for line in input.lines() {
                    let t = line.trim();
                    if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                        let path_part = inner.split('#').next().unwrap_or(inner).trim();
                        if !path_part.is_empty() {
                            accesses.push(FileAccess {
                                path: super::sanitize_terminal_text(path_part),
                                operation: FileOp::Edit,
                                turn_index,
                            });
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

fn parse_entry_timestamp_ms(val: &Value) -> u64 {
    if let Some(ts) = val.get("timestamp").and_then(Value::as_str) {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
            return dt.timestamp_millis().max(0) as u64;
        }
    }
    if let Some(ms) = val
        .get("message")
        .and_then(|m| m.get("timestamp"))
        .and_then(Value::as_u64)
    {
        return ms;
    }
    0
}

fn extract_message_text(msg: &Value, max_chars: usize) -> String {
    let raw = match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    b.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    };
    clean_text(&raw, max_chars)
}

fn clean_text(raw: &str, max_chars: usize) -> String {
    let joined = raw
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("```")
                && !l.starts_with("<system-")
                && !l.starts_with("</system-")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let safe = super::sanitize_terminal_text(joined.trim());
    let redacted = super::redact_secrets(&safe);
    truncate_chars(&redacted, max_chars)
}

fn push_chat_message(messages: &mut Vec<ChatMessage>, role: ChatRole, text: String) {
    if text.is_empty() {
        return;
    }
    messages.push(ChatMessage { role, text });
    if messages.len() > MAX_CHAT_MESSAGES {
        let excess = messages.len() - MAX_CHAT_MESSAGES;
        messages.drain(..excess);
    }
}

fn shorten_path(path: &str) -> String {
    #[cfg(windows)]
    let parts: Vec<&str> = path.rsplit(['/', '\\']).collect();
    #[cfg(not(windows))]
    let parts: Vec<&str> = path.rsplit('/').collect();
    if parts.len() <= 2 {
        path.to_string()
    } else {
        format!("{}/{}", parts[1], parts[0])
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}…")
    }
}

fn session_id_from_filename(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if let Some((_, sid)) = stem.rsplit_once('_') {
        if !sid.is_empty() {
            return sid.to_string();
        }
    }
    stem.to_string()
}

fn classify_open_session_path(path: &Path, agent_dirs: &[PathBuf]) -> Option<(PathBuf, PathBuf)> {
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return None;
    }
    let project_dir = path.parent()?;
    let sessions_dir = project_dir.parent()?;
    if sessions_dir.file_name().and_then(|n| n.to_str()) != Some("sessions") {
        // Could be a subagent or eval log inside `<sessions>/<project>/<session_stem>/...`
        let grand = sessions_dir.parent()?;
        if grand.file_name().and_then(|n| n.to_str()) == Some("sessions") {
            let root_jsonl = project_dir.with_extension("jsonl");
            let agent_dir = grand.parent()?.to_path_buf();
            if agent_dirs.is_empty() || agent_dirs.iter().any(|d| d == &agent_dir) {
                return Some((root_jsonl, agent_dir));
            }
        }
        return None;
    }
    let agent_dir = sessions_dir.parent()?.to_path_buf();
    if agent_dirs.is_empty() || agent_dirs.iter().any(|d| d == &agent_dir) {
        return Some((path.to_path_buf(), agent_dir));
    }
    None
}

fn infer_agent_dir_from_session_path(session_path: &Path, known_dirs: &[PathBuf]) -> PathBuf {
    for ancestor in session_path.ancestors() {
        if ancestor.file_name().and_then(|n| n.to_str()) == Some("sessions") {
            if let Some(parent) = ancestor.parent() {
                return parent.to_path_buf();
            }
        }
    }
    known_dirs
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from("."))
}

fn extract_cli_session_arg(command: &str) -> Option<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    for (idx, &tok) in tokens.iter().enumerate() {
        if matches!(tok, "--resume" | "-r" | "--session") {
            if let Some(&val) = tokens.get(idx + 1) {
                if !val.starts_with('-') {
                    return Some(val.trim_matches(['"', '\'']).to_string());
                }
            }
        } else if let Some(val) = tok
            .strip_prefix("--resume=")
            .or_else(|| tok.strip_prefix("--session="))
        {
            if !val.is_empty() {
                return Some(val.trim_matches(['"', '\'']).to_string());
            }
        }
    }
    None
}

fn read_terminal_session_file(path: &Path) -> Option<PathBuf> {
    if is_symlink(path) {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let _cwd = lines.next()?;
    let session_file = lines.next()?.trim();
    if session_file.is_empty() {
        return None;
    }
    Some(PathBuf::from(session_file))
}

fn candidate_cwd_session_dirs(sessions_root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let cwd_str = cwd.to_string_lossy();
    let trimmed = cwd_str.trim_start_matches(['/', '\\']);
    let dashed: String = trimmed
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            _ => c,
        })
        .collect();
    // Pi format: `--<full-path-with-dashes>--`
    dirs.push(sessions_root.join(format!("--{dashed}--")));

    // OMP format: when under home dir, `-<rel-path-with-dashes>`
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = cwd.strip_prefix(&home) {
            let rel_str = rel.to_string_lossy();
            if rel_str.is_empty() {
                dirs.push(sessions_root.join("-"));
            } else {
                let rel_dashed: String = rel_str
                    .chars()
                    .map(|c| match c {
                        '/' | '\\' | ':' => '-',
                        _ => c,
                    })
                    .collect();
                dirs.push(sessions_root.join(format!("-{rel_dashed}")));
            }
        }
    }
    dirs
}

fn find_file_with_suffix_or_name(dir: &Path, sid: &str, suffix: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let exact = format!("{sid}.jsonl");
    for entry in entries.flatten() {
        if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(true) {
            continue;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if (name == exact || name.ends_with(suffix)) && path.is_file() {
            return Some(path);
        }
    }
    None
}

fn most_recent_jsonl_in_dir(dir: &Path, claimed_files: &HashSet<PathBuf>) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(true) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl")
            || claimed_files.contains(&path)
        {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let Ok(mtime) = meta.modified() else {
            continue;
        };
        if best.as_ref().is_none_or(|(b_mtime, _)| mtime > *b_mtime) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, p)| p)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(true)
}

fn map_pid_to_open_paths(pids: &[u32]) -> HashMap<u32, ProcessOpenPaths> {
    if pids.is_empty() {
        return HashMap::new();
    }

    #[cfg(target_os = "linux")]
    {
        map_pid_to_proc_open_paths(pids)
    }

    #[cfg(target_vendor = "apple")]
    {
        map_pid_to_apple_open_paths(pids)
    }

    #[cfg(target_os = "windows")]
    {
        map_pid_to_windows_open_paths(pids)
    }

    #[cfg(all(
        not(target_os = "linux"),
        not(target_vendor = "apple"),
        not(target_os = "windows")
    ))]
    {
        map_pid_to_lsof_open_paths(pids)
    }
}

#[cfg(target_os = "linux")]
fn map_pid_to_proc_open_paths(pids: &[u32]) -> HashMap<u32, ProcessOpenPaths> {
    let mut map = HashMap::new();
    for &pid in pids {
        let cwd = fs::read_link(format!("/proc/{pid}/cwd")).ok();
        let mut paths = Vec::new();
        let mut ttys = Vec::new();
        if let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) {
            for entry in entries.flatten() {
                if let Ok(target) = fs::read_link(entry.path()) {
                    if let Some(tty_name) = target
                        .to_str()
                        .and_then(|s| s.strip_prefix("/dev/"))
                        .filter(|s| !s.is_empty())
                    {
                        let cleaned = tty_name.replace('/', "_");
                        if !ttys.contains(&cleaned) {
                            ttys.push(cleaned);
                        }
                    }
                    if target.is_absolute() {
                        paths.push(target);
                    }
                }
            }
        }
        map.insert(pid, ProcessOpenPaths { cwd, paths, ttys });
    }
    map
}

#[cfg(target_vendor = "apple")]
fn map_pid_to_apple_open_paths(pids: &[u32]) -> HashMap<u32, ProcessOpenPaths> {
    use proc_pidinfo::{
        proc_pidfdinfo, proc_pidinfo_list, Pid, ProcFDInfo, ProcFDType, VnodeFdInfoWithPath,
    };

    let mut map = HashMap::new();
    for &raw_pid in pids {
        let pid = Pid(raw_pid);
        let Ok(fds) = proc_pidinfo_list::<ProcFDInfo>(pid) else {
            continue;
        };
        let mut paths = Vec::new();
        let mut ttys = Vec::new();
        for fd in fds {
            if fd.fd_type() != Ok(ProcFDType::VNODE) {
                continue;
            }
            let Some(vnode) = proc_pidfdinfo::<VnodeFdInfoWithPath>(pid, fd.proc_fd)
                .ok()
                .flatten()
            else {
                continue;
            };
            let Ok(path) = vnode.path() else {
                continue;
            };
            if let Some(tty_name) = path
                .to_str()
                .and_then(|s| s.strip_prefix("/dev/tty"))
                .map(|s| format!("tty{s}"))
            {
                if !ttys.contains(&tty_name) {
                    ttys.push(tty_name);
                }
            }
            paths.push(path.to_path_buf());
        }
        map.insert(
            raw_pid,
            ProcessOpenPaths {
                cwd: None,
                paths,
                ttys,
            },
        );
    }
    map
}

#[cfg(target_os = "windows")]
fn map_pid_to_windows_open_paths(pids: &[u32]) -> HashMap<u32, ProcessOpenPaths> {
    use std::sync::{LazyLock, Mutex};
    static SYS: LazyLock<Mutex<sysinfo::System>> =
        LazyLock::new(|| Mutex::new(sysinfo::System::new()));
    let mut sys = SYS.lock().expect("pi open-paths system mutex poisoned");

    let pids_sys: Vec<sysinfo::Pid> = pids
        .iter()
        .copied()
        .map(|p| sysinfo::Pid::from(p as usize))
        .collect();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&pids_sys),
        true,
        sysinfo::ProcessRefreshKind::new().with_cwd(sysinfo::UpdateKind::Always),
    );

    let mut map = HashMap::new();
    for &pid_u32 in pids {
        let pid = sysinfo::Pid::from(pid_u32 as usize);
        if let Some(proc_) = sys.process(pid) {
            let cwd = proc_.cwd().map(PathBuf::from);
            map.insert(
                pid_u32,
                ProcessOpenPaths {
                    cwd,
                    paths: Vec::new(),
                    ttys: Vec::new(),
                },
            );
        }
    }
    map
}

#[cfg(all(
    not(target_os = "linux"),
    not(target_vendor = "apple"),
    not(target_os = "windows")
))]
fn map_pid_to_lsof_open_paths(pids: &[u32]) -> HashMap<u32, ProcessOpenPaths> {
    let pid_args: Vec<String> = pids.iter().map(|p| format!("-p{p}")).collect();
    let mut args = vec!["-F", "ftn"];
    for pa in &pid_args {
        args.push(pa);
    }
    let output = Command::new("lsof").args(&args).output().ok();
    let mut map: HashMap<u32, ProcessOpenPaths> = HashMap::new();
    let Some(out) = output else {
        return map;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut current_pid: Option<u32> = None;
    let mut current_fd = String::new();
    for line in stdout.lines() {
        if let Some(pid_str) = line.strip_prefix('p') {
            current_pid = pid_str.parse::<u32>().ok();
            current_fd.clear();
        } else if let Some(fd) = line.strip_prefix('f') {
            current_fd = fd.to_string();
        } else if let Some(name) = line.strip_prefix('n') {
            let Some(pid) = current_pid else { continue };
            let entry = map.entry(pid).or_default();
            if current_fd == "cwd" {
                entry.cwd = Some(PathBuf::from(name));
            } else if let Some(tty_name) = name.strip_prefix("/dev/tty") {
                let tty = format!("tty{tty_name}");
                if !entry.ttys.contains(&tty) {
                    entry.ttys.push(tty);
                }
            }
            entry.paths.push(PathBuf::from(name));
        }
    }
    map
}

#[cfg(target_os = "linux")]
fn get_process_cwd(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(target_os = "windows")]
fn get_process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
fn get_process_cwd(pid: u32) -> Option<PathBuf> {
    let output = std::process::Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|l| l.strip_prefix('n'))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn resolve_git_branch(cwd: &str) -> String {
    if !Path::new(cwd).is_dir() {
        return String::new();
    }
    let output = std::process::Command::new("git")
        .args(["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok();
    if let Some(output) = output {
        if output.status.success() {
            return super::sanitize_terminal_text(String::from_utf8_lossy(&output.stdout).trim());
        }
    }
    String::new()
}

fn resolve_harness_version(
    agent_cli: &str,
    agent_dir: &Path,
    proc_open: Option<&ProcessOpenPaths>,
    cache: &mut HashMap<PathBuf, String>,
) -> String {
    // 1. Check open native module paths for `.omp/natives/<ver>/`
    if agent_cli == "omp" {
        if let Some(open) = proc_open {
            for path in &open.paths {
                let comps: Vec<&str> = path
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect();
                for window in comps.windows(2) {
                    if window[0] == "natives" && !window[1].is_empty() {
                        return super::sanitize_terminal_text(window[1]);
                    }
                }
            }
        }
    }

    if let Some(v) = cache.get(agent_dir) {
        return v.clone();
    }

    let ver = if agent_cli == "pi" {
        fs::read_to_string(agent_dir.join("install").join("current-version"))
            .ok()
            .or_else(|| fs::read_to_string(agent_dir.join("last-changelog-version")).ok())
            .map(|s| super::sanitize_terminal_text(s.trim()))
            .unwrap_or_default()
    } else {
        fs::read_to_string(agent_dir.join("last-changelog-version"))
            .ok()
            .map(|s| super::sanitize_terminal_text(s.trim()))
            .unwrap_or_default()
    };
    cache.insert(agent_dir.to_path_buf(), ver.clone());
    ver
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn filters_omp_background_workers_and_keeps_interactive_sessions() {
        let collector = PiCollector::new_omp();
        let mut procs = HashMap::new();
        procs.insert(
            10,
            ProcInfo {
                pid: 10,
                ppid: 1,
                rss_kb: 1000,
                cpu_pct: 0.0,
                command: "/opt/homebrew/bin/omp".to_string(),
            },
        );
        procs.insert(
            11,
            ProcInfo {
                pid: 11,
                ppid: 10,
                rss_kb: 500,
                cpu_pct: 0.0,
                command: "/opt/homebrew/bin/omp __omp_worker_daemon_broker".to_string(),
            },
        );
        procs.insert(
            12,
            ProcInfo {
                pid: 12,
                ppid: 10,
                rss_kb: 500,
                cpu_pct: 0.0,
                command: "/opt/homebrew/bin/omp __omp_worker_js_eval_process".to_string(),
            },
        );

        let pids = collector.find_pids(&procs, 999);
        assert_eq!(pids, vec![10]);
    }

    #[test]
    fn deduplicates_pi_launcher_and_node_child() {
        let collector = PiCollector::new_pi();
        let mut procs = HashMap::new();
        procs.insert(
            100,
            ProcInfo {
                pid: 100,
                ppid: 1,
                rss_kb: 1000,
                cpu_pct: 0.0,
                command: "/bin/sh /Users/test/.pi/agent/bin/pi".to_string(),
            },
        );
        procs.insert(
            101,
            ProcInfo {
                pid: 101,
                ppid: 100,
                rss_kb: 50_000,
                cpu_pct: 1.0,
                command: "node /Users/test/.pi/agent/install/releases/1.0.0/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js".to_string(),
            },
        );

        let pids = collector.find_pids(&procs, 999);
        assert_eq!(pids, vec![101]);
    }

    #[test]
    fn skips_pi_subcommands_through_launcher_and_node_child() {
        let collector = PiCollector::new_pi();
        let entry = "/Users/test/.pi/agent/install/releases/1.0.0/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js";
        for (launcher, child) in [
            (
                "/bin/sh /Users/test/.pi/agent/bin/pi update".to_string(),
                format!("node {entry} update"),
            ),
            (
                "pi config".to_string(),
                "node /Users/test/proj/node_modules/.bin/pi config".to_string(),
            ),
        ] {
            let mut procs = HashMap::new();
            for (pid, ppid, command) in [(100, 1, launcher), (101, 100, child)] {
                procs.insert(
                    pid,
                    ProcInfo {
                        pid,
                        ppid,
                        rss_kb: 1000,
                        cpu_pct: 0.0,
                        command,
                    },
                );
            }
            assert!(collector.find_pids(&procs, 999).is_empty());
        }

        // A prompt that happens to be a subcommand name is not the subcommand position.
        let mut procs = HashMap::new();
        procs.insert(
            101,
            ProcInfo {
                pid: 101,
                ppid: 1,
                rss_kb: 1000,
                cpu_pct: 0.0,
                command: format!("node {entry} --model x update"),
            },
        );
        assert_eq!(collector.find_pids(&procs, 999), vec![101]);
    }

    #[test]
    fn parses_omp_and_pi_transcript_usage_context_and_tools() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir
            .path()
            .join("2026-10-15T17-03-03-461Z_01a14084-6f25-75ae-8039-f212838cad47.jsonl");
        let mut f = fs::File::create(&jsonl).unwrap();
        writeln!(
            f,
            r#"{{"type":"title","v":1,"title":"Add omp and pi support","source":"auto"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"session","version":3,"id":"01a14084-6f25-75ae-8039-f212838cad47","timestamp":"2026-10-15T17:03:03.461Z","cwd":"/Users/test/abtop"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"model_change","id":"1","timestamp":"2026-10-15T17:03:03.500Z","model":"fuelix/gemini-4-argon"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"thinking_level_change","id":"2","timestamp":"2026-10-15T17:03:03.500Z","thinkingLevel":"low"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"message","id":"3","timestamp":"2026-10-15T17:03:14.000Z","message":{{"role":"user","content":[{{"type":"text","text":"Add support for omp and pi"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"message","id":"4","timestamp":"2026-10-15T17:03:18.000Z","message":{{"role":"assistant","provider":"fuelix","model":"gemini-4-argon","content":[{{"type":"toolCall","id":"call_1","name":"read","arguments":{{"path":"src/collector/mod.rs"}},"intent":"Reading collector mod"}}],"usage":{{"input":1000,"output":50,"cacheRead":2000,"cacheWrite":500}},"contextSnapshot":{{"promptTokens":3000,"nonMessageTokens":500}}}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"message","id":"5","timestamp":"2026-10-15T17:03:20.000Z","message":{{"role":"toolResult","toolCallId":"call_1","toolName":"read","content":[{{"type":"text","text":"ok"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"message","id":"6","timestamp":"2026-10-15T17:03:25.000Z","message":{{"role":"assistant","provider":"fuelix","model":"gemini-4-argon","content":[{{"type":"text","text":"Done!"}}],"usage":{{"input":1200,"output":80,"cacheRead":2500,"cacheWrite":0}}}}}}"#
        )
        .unwrap();

        let mut state = TranscriptState::default();
        update_transcript_state(&jsonl, &mut state);

        assert_eq!(state.session_id, "01a14084-6f25-75ae-8039-f212838cad47");
        assert_eq!(state.cwd, "/Users/test/abtop");
        assert_eq!(state.title, "Add omp and pi support");
        assert_eq!(state.model, "gemini-4-argon");
        assert_eq!(state.effort, "low");
        assert_eq!(state.turn_count, 2);
        assert_eq!(state.total_input, 2200);
        assert_eq!(state.total_output, 130);
        assert_eq!(state.total_cache_read, 4500);
        assert_eq!(state.total_cache_create, 500);
        assert_eq!(state.last_context_tokens, 3700);
        assert_eq!(state.tool_calls.len(), 1);
        assert_eq!(state.tool_calls[0].name, "Read");
        assert_eq!(state.tool_calls[0].duration_ms, 2000);
        assert_eq!(state.file_accesses.len(), 1);
        assert!(state.current_task.is_empty());
        assert_eq!(state.thinking_since_ms, 0);
    }
}
