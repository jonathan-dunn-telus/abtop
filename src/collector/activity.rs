//! Small, incremental lifecycle readers. No prompts or tool results are retained.
use crate::model::AgentCounts;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
#[cfg(not(unix))]
use std::time::UNIX_EPOCH;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum State {
    Working,
    Done,
    #[default]
    Unknown,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Done => "done",
            Self::Unknown => "unknown",
        }
    }

    fn count(self, counts: &mut AgentCounts) {
        match self {
            Self::Working => counts.working += 1,
            Self::Unknown => counts.unknown += 1,
            Self::Done => {}
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Event {
    state: State,
    at: i64,
}

fn timestamp(v: &Value) -> i64 {
    v["timestamp"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

fn identity(meta: &fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        (
            0,
            meta.created()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0),
        )
    }
}

pub(super) struct Tail<T> {
    pub data: T,
    offset: u64,
    identity: (u64, u64),
    pub readable: bool,
}

impl<T: Default> Default for Tail<T> {
    fn default() -> Self {
        Self {
            data: T::default(),
            offset: 0,
            identity: (0, 0),
            readable: false,
        }
    }
}

impl<T: Default> Tail<T> {
    pub fn update(&mut self, path: &Path, mut consume: impl FnMut(&mut T, &Value)) {
        self.readable = false;
        let Ok(meta) = fs::symlink_metadata(path) else {
            return;
        };
        if !meta.is_file() {
            return;
        }
        let id = identity(&meta);
        if id != self.identity || meta.len() < self.offset {
            *self = Self::default();
            self.identity = id;
        }
        self.readable = true;
        if meta.len() == self.offset {
            return;
        }
        let Ok(mut file) = File::open(path) else {
            self.readable = false;
            return;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            self.readable = false;
            return;
        }
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            // Re-read incomplete lines next refresh; physically bound malformed input.
            let read = reader
                .by_ref()
                .take(10 * 1024 * 1024 + 1)
                .read_until(b'\n', &mut line);
            match read {
                Ok(0) => break,
                Ok(_) if line.len() > 10 * 1024 * 1024 => {
                    self.readable = false;
                    break;
                }
                Ok(_) if line.last() != Some(&b'\n') => break,
                Ok(n) => {
                    self.offset += n as u64;
                    if let Ok(v) = serde_json::from_slice(&line) {
                        consume(&mut self.data, &v);
                    }
                }
                Err(_) => {
                    self.readable = false;
                    break;
                }
            }
        }
    }
}

/// Enumerate files without following directory or file symlinks.
pub(super) fn files_under(root: &Path, output: &mut Vec<PathBuf>) {
    if !fs::symlink_metadata(root).is_ok_and(|m| m.is_dir()) {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            files_under(&entry.path(), output);
        } else if kind.is_file() {
            output.push(entry.path());
        }
    }
}

#[derive(Default)]
struct Parent {
    agents: HashMap<String, Event>,
    workflows: HashMap<String, (String, Event)>, // run ID -> task ID and state
    tasks: HashMap<String, Event>,
    calls: HashMap<String, (String, Option<String>)>,
}

impl Parent {
    fn workflow_closed(&self, run: &str) -> bool {
        self.workflows.get(run).is_some_and(|(task, launch)| {
            launch.state == State::Done
                || self
                    .tasks
                    .get(task)
                    .is_some_and(|end| end.at >= launch.at && end.state == State::Done)
        })
    }
}

#[derive(Default)]
struct Child {
    own: Event,
    nested: Parent,
}

fn state_from_status(s: &str) -> Option<State> {
    match s {
        "completed" | "stopped" | "killed" | "failed" | "error" | "done" => Some(State::Done),
        "async_launched" | "running" => Some(State::Working),
        _ => None,
    }
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = format!("<{name}>");
    let end = format!("</{name}>");
    text.split_once(&start)?.1.split_once(&end).map(|v| v.0)
}

fn parent_event(parent: &mut Parent, v: &Value) {
    let at = timestamp(v);
    let content = &v["message"]["content"];
    if v["type"] == "assistant" {
        if let Some(blocks) = content.as_array() {
            for block in blocks {
                if block["type"] == "tool_use" {
                    if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                        if matches!(name, "Agent" | "Task" | "Workflow" | "TaskStop") {
                            parent.calls.insert(
                                id.to_owned(),
                                (
                                    name.to_owned(),
                                    block["input"]["task_id"].as_str().map(str::to_owned),
                                ),
                            );
                        }
                    }
                }
            }
        }
        return;
    }
    if v["type"] != "user" {
        return;
    }
    let r = &v["toolUseResult"];
    let blocks = content.as_array();
    let correlated = blocks.into_iter().flatten().find_map(|b| {
        b["tool_use_id"]
            .as_str()
            .and_then(|id| parent.calls.get(id))
            .cloned()
    });
    if let Some(id) = r["agentId"].as_str() {
        // Skill uses `forked` for both synchronous results and background launches.
        // Only the explicit async flags make that result an open lifecycle.
        let state = state_from_status(r["status"].as_str().unwrap_or("")).unwrap_or(
            if r["isAsync"] == true || r["background"] == true {
                State::Working
            } else {
                State::Done
            },
        );
        parent.agents.insert(id.to_owned(), Event { state, at });
    }
    if let (Some(run), Some(task)) = (r["runId"].as_str(), r["taskId"].as_str()) {
        let state = state_from_status(r["status"].as_str().unwrap_or("")).unwrap_or(State::Working);
        parent
            .workflows
            .insert(run.to_owned(), (task.to_owned(), Event { state, at }));
    }
    if correlated
        .as_ref()
        .is_some_and(|(name, _)| name == "TaskStop")
        && !blocks.into_iter().flatten().any(|b| b["is_error"] == true)
    {
        if let Some(id) = r["task_id"]
            .as_str()
            .or_else(|| correlated.as_ref().and_then(|(_, id)| id.as_deref()))
        {
            parent.tasks.insert(
                id.to_owned(),
                Event {
                    state: State::Done,
                    at,
                },
            );
        }
    }
    let texts = content.as_str().into_iter().chain(
        blocks
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str()),
    );
    for text in texts {
        // Only read the notification envelope, never arbitrary result/summary text.
        if !text.trim_start().starts_with("<task-notification>") {
            continue;
        }
        if let (Some(id), Some(state)) = (
            tag(text, "task-id"),
            tag(text, "status").and_then(state_from_status),
        ) {
            parent.tasks.insert(id.to_owned(), Event { state, at });
        }
    }
    for b in blocks.into_iter().flatten() {
        if let Some(id) = b["tool_use_id"].as_str() {
            parent.calls.remove(id);
        }
    }
}

fn child_event(state: &mut Event, v: &Value) {
    let next = match v["type"].as_str() {
        Some("user") => {
            // StructuredOutput and MCP tools can finish a turn themselves,
            // without a subsequent assistant message carrying end_turn.
            let ends_turn =
                v["toolEndsTurn"] == true || v["mcpMeta"]["_meta"]["claude/endTurn"] == true;
            let failed = v["message"]["content"].as_array().is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|b| b["type"] == "tool_result" && b["is_error"] == true)
            });
            Some(if ends_turn && !failed {
                State::Done
            } else {
                State::Working
            })
        }
        Some("assistant") => match v["message"]["stop_reason"].as_str() {
            Some("end_turn" | "stop_sequence") => Some(State::Done),
            _ => Some(State::Working),
        },
        _ => None,
    };
    if let Some(next) = next {
        *state = Event {
            state: next,
            at: timestamp(v),
        };
    }
}

/// Lifecycle evidence has no file cursor: Claude's usage reader delivers records.
#[derive(Default)]
struct Evidence<T> {
    data: T,
    readable: bool,
    incomplete: bool,
}

impl<T> Evidence<T> {
    fn finish(&mut self, readable: bool, skipped: bool) {
        self.incomplete |= skipped;
        self.readable = readable && !self.incomplete;
    }
}

#[derive(Default)]
pub(super) struct ClaudeActivity {
    parent: Evidence<Parent>,
    children: HashMap<PathBuf, Evidence<Child>>,
    journals: HashMap<PathBuf, Tail<HashMap<String, State>>>,
    pub states: HashMap<String, State>,
    #[cfg(test)]
    cursors: HashMap<PathBuf, (u64, (u64, u64))>,
}

impl ClaudeActivity {
    #[cfg(test)]
    fn update(&mut self, parent_path: &Path, directory: &Path) -> AgentCounts {
        let mut paths = Vec::new();
        files_under(directory, &mut paths);
        for path in std::iter::once(parent_path).chain(
            paths
                .iter()
                .filter(|p| {
                    p.file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.starts_with("agent-") && s.ends_with(".jsonl"))
                })
                .map(PathBuf::as_path),
        ) {
            let (mut offset, old_id) = self.cursors.get(path).copied().unwrap_or_default();
            let id = super::claude::file_identity(path);
            if id != old_id || fs::metadata(path).is_ok_and(|m| m.len() < offset) {
                offset = 0;
                if path == parent_path {
                    self.reset_parent();
                } else {
                    self.reset_child(path);
                }
            }
            let read = super::claude::read_records(path, offset, |v| {
                if path == parent_path {
                    self.parent_event(v);
                } else {
                    self.child_event(path, v);
                }
            });
            if path == parent_path {
                self.parent_read(read.readable, read.skipped);
            } else {
                self.child_read(path, read.readable, read.skipped);
            }
            self.cursors.insert(path.to_path_buf(), (read.offset, id));
        }
        self.reconcile(directory, &paths)
    }

    pub fn reset_parent(&mut self) {
        self.parent = Evidence::default();
    }

    pub fn parent_event(&mut self, value: &Value) {
        parent_event(&mut self.parent.data, value);
    }

    pub fn parent_read(&mut self, readable: bool, skipped: bool) {
        self.parent.finish(readable, skipped);
    }

    pub fn reset_child(&mut self, path: &Path) {
        self.children
            .insert(path.to_path_buf(), Evidence::default());
    }

    pub fn child_event(&mut self, path: &Path, value: &Value) {
        let child = self.children.entry(path.to_path_buf()).or_default();
        child_event(&mut child.data.own, value);
        parent_event(&mut child.data.nested, value);
    }

    pub fn child_read(&mut self, path: &Path, readable: bool, skipped: bool) {
        self.children
            .entry(path.to_path_buf())
            .or_default()
            .finish(readable, skipped);
    }

    pub fn reconcile(&mut self, directory: &Path, paths: &[PathBuf]) -> AgentCounts {
        let found: HashSet<_> = paths.iter().cloned().collect();
        // Keep completed lifecycle evidence even if a transcript is removed.
        self.states.clear();
        for (id, event) in &self.parent.data.agents {
            let event = self
                .parent
                .data
                .tasks
                .get(id)
                .filter(|task| task.at >= event.at)
                .unwrap_or(event);
            self.states.insert(
                id.clone(),
                if self.parent.readable {
                    event.state
                } else {
                    State::Unknown
                },
            );
        }
        for path in paths.iter().filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("agent-") && s.ends_with(".jsonl"))
        }) {
            let id = path
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .trim_start_matches("agent-")
                .to_owned();
            let child = self.children.entry(path.clone()).or_default();
            let mut event = child.data.own;
            let key = activity_key(directory, path.parent().unwrap(), &id);
            if let Some(parent) = self.parent.data.agents.get(&key) {
                if parent.at >= event.at {
                    event = *parent;
                }
            }
            if let Some(task) = self.parent.data.tasks.get(&key) {
                if task.at >= event.at {
                    event = *task;
                }
            }
            self.states.insert(
                key,
                if child.readable {
                    event.state
                } else {
                    State::Unknown
                },
            );
        }
        for path in paths
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "journal.jsonl"))
        {
            let journal = self.journals.entry(path.clone()).or_default();
            journal.update(path, |agents, v| {
                if let Some(id) = v["agentId"].as_str() {
                    let state = match v["type"].as_str() {
                        Some("started") => State::Working,
                        Some("result" | "failed") => State::Done,
                        _ => return,
                    };
                    agents.insert(id.to_owned(), state);
                }
            });
            let run = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let closed = workflow_finished(directory, run)
                || self.parent.data.workflow_closed(run)
                || self
                    .children
                    .values()
                    .any(|c| c.data.nested.workflow_closed(run));
            for (id, state) in &journal.data {
                self.states.insert(
                    activity_key(directory, path.parent().unwrap(), id),
                    if closed {
                        State::Done
                    } else if journal.readable {
                        *state
                    } else {
                        State::Unknown
                    },
                );
            }
            if closed {
                for child_path in self
                    .children
                    .keys()
                    .filter(|child| child.starts_with(path.parent().unwrap()))
                {
                    let id = child_path.file_stem().unwrap().to_string_lossy();
                    self.states.insert(
                        activity_key(
                            directory,
                            child_path.parent().unwrap(),
                            id.trim_start_matches("agent-"),
                        ),
                        State::Done,
                    );
                }
            }
        }
        // Removed open files cannot prove that work is still in progress.
        for (path, child) in &self.children {
            if !found.contains(path) {
                let id = path
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .trim_start_matches("agent-")
                    .to_owned();
                let key = activity_key(directory, path.parent().unwrap(), &id);
                if self.states.get(&key) != Some(&State::Done) {
                    let restarted = self
                        .parent
                        .data
                        .agents
                        .get(&key)
                        .is_some_and(|e| e.at > child.data.own.at && e.state == State::Working);
                    self.states.insert(
                        key,
                        if child.data.own.state == State::Done && !restarted {
                            State::Done
                        } else {
                            State::Unknown
                        },
                    );
                }
            }
        }
        let mut counts = AgentCounts::default();
        for state in self.states.values() {
            state.count(&mut counts);
        }
        counts
    }
}

/// Claude Code writes `<session>/workflows/<runId>.json` only when a run
/// ends, so its presence closes every agent of the run.
pub(super) fn workflow_finished(subagents_dir: &Path, run: &str) -> bool {
    fs::symlink_metadata(
        subagents_dir
            .with_file_name("workflows")
            .join(format!("{run}.json")),
    )
    .is_ok_and(|meta| meta.is_file())
}

pub(super) fn activity_key(root: &Path, directory: &Path, id: &str) -> String {
    let relative = directory.strip_prefix(root).unwrap_or(directory);
    if relative.as_os_str().is_empty() {
        id.to_owned()
    } else {
        format!("{}:{id}", relative.to_string_lossy())
    }
}

#[derive(Default, Clone)]
pub(super) struct CodexMeta {
    pub id: String,
    pub parent: Option<String>,
    history_start: Option<u64>,
}

pub(super) fn codex_meta(path: &Path) -> Option<CodexMeta> {
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?)
        .take(10 * 1024 * 1024)
        .read_line(&mut line)
        .ok()?;
    let v: Value = serde_json::from_str(&line).ok()?;
    if v["type"] != "session_meta" {
        return None;
    }
    let p = &v["payload"];
    Some(CodexMeta {
        id: p["id"]
            .as_str()
            .or_else(|| p["session_id"].as_str())?
            .to_owned(),
        parent: p["source"]["subagent"]["thread_spawn"]["parent_thread_id"]
            .as_str()
            .or_else(|| p["parent_thread_id"].as_str())
            .map(str::to_owned),
        history_start: p["subagent_history_start_ordinal"].as_u64(),
    })
}

#[derive(Default)]
pub(super) struct CodexTurn {
    ordinal: u64,
    state: State,
    turn: Option<String>,
    inherited: bool,
}

impl CodexTurn {
    pub fn consume(&mut self, v: &Value, meta: &CodexMeta) {
        let ordinal = self.ordinal;
        self.ordinal += 1;
        if v["type"] == "session_meta" {
            if ordinal > 0 && meta.history_start.is_none() {
                self.inherited = true;
                self.state = State::Unknown;
            }
            return;
        }
        // The ordinal excludes the owning session_meta at the head of the file.
        if meta.history_start.is_some_and(|start| ordinal <= start) || self.inherited {
            return;
        }
        if v["type"] != "event_msg" {
            return;
        }
        let p = &v["payload"];
        match p["type"].as_str() {
            Some("task_started" | "user_message") => {
                self.state = State::Working;
                if let Some(turn) = p["turn_id"].as_str() {
                    self.turn = Some(turn.to_owned());
                }
            }
            Some("task_complete" | "turn_aborted")
                if p["turn_id"]
                    .as_str()
                    .zip(self.turn.as_deref())
                    .is_none_or(|(a, b)| a == b) =>
            {
                self.state = State::Done;
            }
            _ => {}
        }
    }
}

pub(super) fn count_codex(
    roots: &HashSet<String>,
    metadata: &HashMap<PathBuf, CodexMeta>,
    tails: &mut HashMap<PathBuf, Tail<CodexTurn>>,
) -> HashMap<String, AgentCounts> {
    let parents: HashMap<_, _> = metadata
        .values()
        .filter_map(|m| m.parent.as_ref().map(|p| (m.id.as_str(), p.as_str())))
        .collect();
    let mut seen = HashSet::new();
    let mut counts = HashMap::<String, AgentCounts>::new();
    let mut relevant = HashSet::new();
    // A renamed or rotated rollout may leave an old metadata entry with the
    // same thread ID. Prefer a present, most recently modified file.
    let mut candidates: Vec<_> = metadata
        .iter()
        .filter_map(|(path, meta)| {
            let mut parent = meta.parent.as_deref()?;
            let mut chain = HashSet::from([meta.id.as_str()]);
            while !roots.contains(parent) && chain.insert(parent) {
                let Some(next) = parents.get(parent) else {
                    break;
                };
                parent = next;
            }
            roots.contains(parent).then_some((path, meta, parent))
        })
        .collect();
    candidates.sort_by_cached_key(|(path, _, _)| {
        let stat = fs::symlink_metadata(path).ok().filter(|s| s.is_file());
        std::cmp::Reverse((
            stat.is_some(),
            stat.and_then(|s| s.modified().ok()),
            (*path).clone(),
        ))
    });
    for (path, meta, parent) in candidates {
        if !seen.insert(&meta.id) {
            continue;
        }
        relevant.insert(path.clone());
        let tail = tails.entry(path.clone()).or_default();
        tail.update(path, |state, v| state.consume(v, meta));
        let state = if tail.readable {
            tail.data.state
        } else {
            State::Unknown
        };
        state.count(counts.entry(parent.to_owned()).or_default());
    }
    tails.retain(|p, _| relevant.contains(p));
    counts
}

#[derive(Default)]
pub(super) struct CodexActivity {
    metadata: HashMap<PathBuf, CodexMeta>,
    tails: HashMap<PathBuf, Tail<CodexTurn>>,
    headers: HashMap<PathBuf, CachedHeader>,
    pending: Option<Receiver<HashMap<PathBuf, CachedHeader>>>,
    scanned: Option<Instant>,
}

#[derive(Clone)]
struct CachedHeader {
    identity: (u64, u64),
    len: u64,
    meta: CodexMeta,
}

impl CodexActivity {
    pub fn update(&mut self, directory: &Path, sessions: &mut [crate::model::AgentSession]) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(headers) => {
                    for (path, header) in &headers {
                        self.metadata.insert(path.clone(), header.meta.clone());
                    }
                    self.headers = headers;
                    self.pending = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.pending.is_none()
            && self
                .scanned
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(10))
        {
            let directory = directory.to_path_buf();
            let (tx, rx) = mpsc::channel();
            self.pending = Some(rx);
            self.scanned = Some(Instant::now());
            let previous = self.headers.clone();
            std::thread::spawn(move || {
                let mut paths = Vec::new();
                files_under(&directory, &mut paths);
                let metadata = paths
                    .into_iter()
                    .filter(|p| {
                        p.file_name()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| s.starts_with("rollout-") && s.ends_with(".jsonl"))
                    })
                    .filter_map(|p| {
                        let stat = fs::symlink_metadata(&p).ok()?;
                        let id = identity(&stat);
                        let meta = match previous
                            .get(&p)
                            .filter(|old| old.identity == id && stat.len() >= old.len)
                        {
                            Some(old) => old.meta.clone(),
                            None => codex_meta(&p)?,
                        };
                        Some((
                            p,
                            CachedHeader {
                                identity: id,
                                len: stat.len(),
                                meta,
                            },
                        ))
                    })
                    .collect();
                let _ = tx.send(metadata);
            });
        }
        let roots = sessions
            .iter()
            .filter(|s| {
                s.pid != 0
                    && !matches!(
                        s.status,
                        crate::model::SessionStatus::Done | crate::model::SessionStatus::Unknown
                    )
            })
            .map(|s| s.session_id.clone())
            .collect();
        let counts = count_codex(&roots, &self.metadata, &mut self.tails);
        for session in sessions {
            session.subagent_counts = counts.get(&session.session_id).copied().unwrap_or_default();
        }
    }

    pub fn observe(&mut self, path: PathBuf) {
        if let Some(meta) = codex_meta(&path) {
            self.metadata.insert(path, meta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn append(path: &Path, values: &[Value]) {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for v in values {
            writeln!(f, "{v}").unwrap();
        }
    }

    fn message(kind: &str, time: &str, content: Value, stop: Option<&str>) -> Value {
        json!({"type":kind,"timestamp":time,"message":{"content":content,"stop_reason":stop}})
    }

    #[test]
    fn synchronous_fork_results_close_children_with_or_without_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent.jsonl");
        let children = tmp.path().join("subagents");
        fs::create_dir(&children).unwrap();
        append(
            &children.join("agent-finished.jsonl"),
            &[message(
                "assistant",
                "2020-01-01T00:00:01Z",
                json!([]),
                Some("end_turn"),
            )],
        );
        for id in ["finished", "missing"] {
            append(
                &parent,
                &[json!({
                    "type":"user", "timestamp":"2020-01-01T00:00:02Z",
                    "toolUseResult":{"success":true,"status":"forked","agentId":id,"result":"done"},
                    "message":{"content":[]}
                })],
            );
        }
        let mut activity = ClaudeActivity::default();
        assert_eq!(activity.update(&parent, &children), AgentCounts::default());
        assert_eq!(activity.states["finished"], State::Done);
        assert_eq!(activity.states["missing"], State::Done);

        append(
            &parent,
            &[json!({
                "type":"user", "timestamp":"2020-01-01T00:00:03Z",
                "toolUseResult":{"success":true,"status":"forked","background":true,"agentId":"finished"},
                "message":{"content":[]}
            })],
        );
        assert_eq!(activity.update(&parent, &children).working, 1);
        append(
            &children.join("agent-finished.jsonl"),
            &[message(
                "assistant",
                "2020-01-01T00:00:04Z",
                json!([]),
                Some("end_turn"),
            )],
        );
        assert_eq!(activity.update(&parent, &children), AgentCounts::default());
    }

    #[test]
    fn successful_terminal_tools_close_children_but_errors_continue_work() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent.jsonl");
        append(&parent, &[]);
        let children = tmp.path().join("subagents");
        fs::create_dir(&children).unwrap();
        let child = children.join("agent-one.jsonl");
        let mut activity = ClaudeActivity::default();
        for marker in [
            json!({"toolEndsTurn":true}),
            json!({"mcpMeta":{"_meta":{"claude/endTurn":true}}}),
        ] {
            for is_error in [true, false] {
                append(
                    &child,
                    &[message(
                        "assistant",
                        "2020-01-01T00:00:00Z",
                        json!([
                            {"type":"tool_use","name":"StructuredOutput","id":"tool","input":{}}
                        ]),
                        None,
                    )],
                );
                assert_eq!(activity.update(&parent, &children).working, 1);
                let mut result = message(
                    "user",
                    "2020-01-01T00:00:01Z",
                    json!([
                        {"type":"tool_result","tool_use_id":"tool","is_error":is_error,"content":"result"}
                    ]),
                    None,
                );
                result
                    .as_object_mut()
                    .unwrap()
                    .extend(marker.as_object().unwrap().clone());
                append(&child, &[result]);
                assert_eq!(
                    activity.update(&parent, &children).working,
                    u32::from(is_error)
                );
                assert_eq!(
                    activity.states["one"],
                    if is_error {
                        State::Working
                    } else {
                        State::Done
                    }
                );
            }
        }
    }

    #[test]
    fn claude_quiet_work_completion_resume_and_partial_records() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent.jsonl");
        append(&parent, &[]);
        let children = tmp.path().join("subagents");
        fs::create_dir(&children).unwrap();
        let child = children.join("agent-one.jsonl");
        append(
            &child,
            &[message("user", "2020-01-01T00:00:00Z", json!("work"), None)],
        );
        let mut activity = ClaudeActivity::default();
        assert_eq!(activity.update(&parent, &children).working, 1);
        let offset = activity.cursors[&child].0;
        assert_eq!(activity.update(&parent, &children).working, 1);
        assert_eq!(activity.cursors[&child].0, offset);
        append(
            &child,
            &[message(
                "assistant",
                "2020-01-01T00:00:01Z",
                json!([]),
                Some("end_turn"),
            )],
        );
        assert_eq!(activity.update(&parent, &children), AgentCounts::default());
        let value = message("user", "2020-01-01T00:00:02Z", json!("resume"), None).to_string();
        let mut f = fs::OpenOptions::new().append(true).open(&child).unwrap();
        write!(f, "{}", &value[..20]).unwrap();
        assert_eq!(activity.update(&parent, &children).working, 0);
        writeln!(f, "{}", &value[20..]).unwrap();
        assert_eq!(activity.update(&parent, &children).working, 1);
        fs::remove_file(&child).unwrap();
        assert_eq!(activity.update(&parent, &children).unknown, 1);
    }

    #[test]
    fn workflow_results_and_successful_stop_override_unfinished_child_transcripts() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent.jsonl");
        let children = tmp.path().join("subagents");
        let workflow = children.join("workflows/run-one");
        fs::create_dir_all(&workflow).unwrap();
        let journal = workflow.join("journal.jsonl");
        append(
            &parent,
            &[
                json!({"type":"user","toolUseResult":{"runId":"run-one","taskId":"task-one","status":"running"},"message":{"content":[]}}),
            ],
        );
        append(
            &journal,
            &[
                json!({"type":"started","agentId":"one"}),
                json!({"type":"started","agentId":"two"}),
            ],
        );
        append(
            &workflow.join("agent-one.jsonl"),
            &[message("user", "2020-01-01T00:00:00Z", json!("work"), None)],
        );
        let mut activity = ClaudeActivity::default();
        assert_eq!(activity.update(&parent, &children).working, 2);
        append(
            &journal,
            &[json!({"type":"result","agentId":"one","result":"private result"})],
        );
        assert_eq!(activity.update(&parent, &children).working, 1);
        append(
            &parent,
            &[
                json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"TaskStop","id":"stop","input":{"task_id":"task-one"}}]}}),
                json!({"type":"user","toolUseResult":"Stopped","message":{"content":[{"type":"tool_result","tool_use_id":"stop"}]}}),
            ],
        );
        assert_eq!(activity.update(&parent, &children), AgentCounts::default());
        assert_eq!(
            activity.states[&activity_key(&children, &workflow, "two")],
            State::Done
        );
    }

    #[test]
    fn async_launch_is_not_completion_and_notification_closes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent.jsonl");
        append(
            &parent,
            &[
                json!({"type":"user","toolUseResult":{"agentId":"one","status":"async_launched","isAsync":true},"message":{"content":[]}}),
            ],
        );
        let mut activity = ClaudeActivity::default();
        assert_eq!(
            activity
                .update(&parent, &tmp.path().join("subagents"))
                .working,
            1
        );
        append(&parent, &[message("user", "2020-01-01T00:00:01Z", json!("<task-notification><task-id>one</task-id><status>completed</status></task-notification>"), None)]);
        assert_eq!(
            activity
                .update(&parent, &tmp.path().join("subagents"))
                .working,
            0
        );
    }

    #[test]
    fn tail_resets_on_shrink_replacement_and_skips_malformed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("child");
        append(
            &path,
            &[json!({"type":"user","message":{"content":"long work assignment"}})],
        );
        let mut tail = Tail::<Event>::default();
        tail.update(&path, child_event);
        assert_eq!(tail.data.state, State::Working);
        fs::write(&path, "invalid\n{}\n").unwrap();
        tail.update(&path, child_event);
        assert_eq!(tail.data.state, State::Unknown);
        let replacement = tmp.path().join("replacement");
        append(
            &replacement,
            &[json!({"type":"assistant","message":{"stop_reason":"end_turn"}})],
        );
        fs::rename(replacement, &path).unwrap();
        tail.update(&path, child_event);
        assert_eq!(tail.data.state, State::Done);
    }

    #[test]
    fn workflow_ids_are_scoped_and_failure_closes_only_its_own_child() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent.jsonl");
        append(&parent, &[]);
        let directory = tmp.path().join("subagents");
        for run in ["a", "b"] {
            let dir = directory.join("workflows").join(run);
            fs::create_dir_all(&dir).unwrap();
            append(
                &dir.join("journal.jsonl"),
                &[json!({"type":"started","agentId":"same"})],
            );
            append(
                &dir.join("agent-same.jsonl"),
                &[message("user", "2020-01-01T00:00:00Z", json!("work"), None)],
            );
        }
        let mut activity = ClaudeActivity::default();
        assert_eq!(activity.update(&parent, &directory).working, 2);
        append(
            &directory.join("workflows/a/journal.jsonl"),
            &[json!({"type":"failed","agentId":"same"})],
        );
        assert_eq!(activity.update(&parent, &directory).working, 1);
        assert_eq!(
            activity.states[&activity_key(&directory, &directory.join("workflows/a"), "same")],
            State::Done
        );
        assert_eq!(
            activity.states[&activity_key(&directory, &directory.join("workflows/b"), "same")],
            State::Working
        );
    }

    #[test]
    fn deleting_completed_child_does_not_resurrect_its_async_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("parent.jsonl");
        let directory = tmp.path().join("subagents");
        fs::create_dir(&directory).unwrap();
        let child = directory.join("agent-one.jsonl");
        append(
            &parent,
            &[
                json!({"type":"user","toolUseResult":{"agentId":"one","status":"async_launched"},"message":{"content":[]}}),
            ],
        );
        append(
            &child,
            &[message(
                "assistant",
                "2020-01-01T00:00:01Z",
                json!([]),
                Some("end_turn"),
            )],
        );
        let mut activity = ClaudeActivity::default();
        assert_eq!(activity.update(&parent, &directory).working, 0);
        fs::remove_file(child).unwrap();
        assert_eq!(activity.update(&parent, &directory), AgentCounts::default());
    }

    #[test]
    fn codex_moved_rollout_wins_over_retained_missing_path() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("rollout-old.jsonl");
        append(
            &old,
            &[
                codex_header("child", Some("root"), None),
                codex_event("task_complete", "one"),
            ],
        );
        let mut metadata = HashMap::from([(old.clone(), codex_meta(&old).unwrap())]);
        let new = tmp.path().join("rollout-new.jsonl");
        fs::rename(&old, &new).unwrap();
        append(&new, &[codex_event("task_started", "two")]);
        metadata.insert(new.clone(), codex_meta(&new).unwrap());
        for _ in 0..10 {
            let mut tails = HashMap::new();
            let counts = count_codex(&HashSet::from(["root".to_owned()]), &metadata, &mut tails);
            assert_eq!(
                counts["root"],
                AgentCounts {
                    working: 1,
                    unknown: 0
                }
            );
        }
    }

    fn codex_header(id: &str, parent: Option<&str>, boundary: Option<u64>) -> Value {
        json!({"type":"session_meta","payload":{"id":id,"source":{"subagent":{"thread_spawn":{"parent_thread_id":parent}}},"subagent_history_start_ordinal":boundary}})
    }

    fn codex_event(kind: &str, turn: &str) -> Value {
        json!({"type":"event_msg","payload":{"type":kind,"turn_id":turn}})
    }

    #[test]
    fn codex_inherited_history_start_complete_resume_abort_and_nesting() {
        let tmp = tempfile::tempdir().unwrap();
        let child = tmp.path().join("rollout-child.jsonl");
        append(
            &child,
            &[
                codex_header("child", Some("root"), Some(3)),
                codex_header("root", None, None),
                codex_event("task_started", "inherited"),
                codex_event("task_complete", "inherited"),
                codex_event("task_started", "own"),
            ],
        );
        let nested = tmp.path().join("rollout-nested.jsonl");
        append(
            &nested,
            &[
                codex_header("nested", Some("child"), None),
                codex_event("task_started", "nested-turn"),
            ],
        );
        let metadata = HashMap::from([
            (child.clone(), codex_meta(&child).unwrap()),
            (nested.clone(), codex_meta(&nested).unwrap()),
        ]);
        assert_eq!(metadata[&child].id, "child");
        let roots = HashSet::from(["root".to_owned()]);
        let mut tails = HashMap::new();
        assert_eq!(
            count_codex(&roots, &metadata, &mut tails)["root"].working,
            2
        );
        append(&child, &[codex_event("task_complete", "inherited")]);
        assert_eq!(
            count_codex(&roots, &metadata, &mut tails)["root"].working,
            2
        );
        append(&child, &[codex_event("task_complete", "own")]);
        assert_eq!(
            count_codex(&roots, &metadata, &mut tails)["root"].working,
            1
        );
        append(&child, &[codex_event("task_started", "second")]);
        assert_eq!(
            count_codex(&roots, &metadata, &mut tails)["root"].working,
            2
        );
        append(&child, &[codex_event("turn_aborted", "second")]);
        assert_eq!(
            count_codex(&roots, &metadata, &mut tails)["root"].working,
            1
        );
        assert!(count_codex(&HashSet::new(), &metadata, &mut tails).is_empty());
        assert!(tails.is_empty());
    }

    #[test]
    fn codex_ambiguous_inherited_history_is_unknown_and_cycles_do_not_hang() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rollout-child.jsonl");
        append(
            &path,
            &[
                codex_header("child", Some("root"), None),
                codex_header("root", None, None),
                codex_event("task_started", "inherited"),
            ],
        );
        let mut metadata = HashMap::from([(path.clone(), codex_meta(&path).unwrap())]);
        let mut tails = HashMap::new();
        assert_eq!(
            count_codex(&HashSet::from(["root".to_owned()]), &metadata, &mut tails)["root"].unknown,
            1
        );
        metadata.get_mut(&path).unwrap().parent = Some("child".to_owned());
        assert!(count_codex(&HashSet::from(["root".to_owned()]), &metadata, &mut tails).is_empty());
    }
}
