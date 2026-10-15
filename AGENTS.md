# abtop

AI agent monitor for your terminal. Like btop++, but for AI coding agents.

Supports Claude Code, Codex CLI, OpenCode, OMP, and Pi sessions.

> **Fork:** this repo is the `raphberube/abtop` fork, and its default branch is `remix`. Releases, merges from downstream (`graykode/abtop`), and GitHub Actions follow [docs/remix-release.md](docs/remix-release.md), which overrides the *Release Process* section below.
>
> Fork features are listed in the README's *Remix features* section and detailed in [docs/remix-features.md](docs/remix-features.md); update both when a remix feature ships or changes. Regenerate the screenshots in `assets/remix/` with `scripts/remix-screenshots.sh`. It captures `abtop --demo` only, so give a new feature synthetic data in `src/demo.rs` rather than capturing real sessions.

## Language Policy

English is mandatory for all project-facing work and communication.

- Write all source code, comments, tests, fixtures, documentation, examples, configuration text, scripts, and user-facing strings in English.
- Use English for every GitHub artifact: issue titles and bodies, issue comments, pull request titles and descriptions, review comments, commit messages, branch names, release notes, changelogs, discussions, labels, milestones, and workflow or CI messages.
- Do not use non-English text in repository content or GitHub communication unless it is an exact external identifier, a required protocol value, or a direct quote needed for context.
- When quoting or preserving non-English input, add an English explanation and keep the non-English text as short as possible.
- If a contributor opens an issue, comment, or review in another language, respond in English and continue the thread in English.

## Architecture

```
src/
├── main.rs                 # Entry, terminal setup, event loop, --setup flag
├── app.rs                  # App state, tick logic, key handling, summary generation
├── setup.rs                # StatusLine hook installation (abtop --setup)
├── ui/
│   ├── mod.rs              # Panel layout and rendering orchestration
│   └── models.rs           # Scoped model token totals, shares, and rates
├── collector/
│   ├── mod.rs              # MultiCollector orchestration, orphan port detection
│   ├── claude.rs           # Claude Code: session discovery, transcript parsing
│   ├── codex.rs            # Codex CLI: session discovery via ps+lsof, JSONL parsing
│   ├── opencode.rs         # OpenCode: session discovery via ps + SQLite DB parsing
│   ├── pi.rs               # OMP and Pi: session discovery via ps + open files, JSONL parsing
│   ├── process.rs          # Child process tree (ps) + open ports (lsof) + git stats
│   └── rate_limit.rs       # Rate limit file reading (~/.claude/abtop-rate-limits.json)
└── model/
    ├── mod.rs              # Re-exports
    └── session.rs          # AgentSession, SessionStatus, RateLimitInfo,
                            # ChildProcess, OrphanPort, SubAgent
```

## Layout

```
┌─ ⁹token rate ──────────────────────────┐┌─ ¹context ───────────────┐
│ Global 25k/min                         ││ abtop       ████████ 82% │
│ ▁▃▅▇█▇▅▃▁▃▅▇██                         ││ prediction  █████████91%⚠│
│ 200-point history                      ││ api-server  ███      22% │
└────────────────────────────────────────┘└──────────────────────────┘
┌─ ²quota ─────┐┌─ ³tokens ───┐┌─ projects ───┐┌─ ⁴ports ──────────┐
│ CLAUDE       ││ Total  1.2M ││ abtop        ││ PORT  SESSION  CMD │
│ 5h ████ 35%  ││ Input  402k ││  main +3 ~18 ││ :3000 api-srv node│
│   resets 2h  ││ Output  89k ││              ││ :8080 predict crgo│
│ 7d ██ 12%    ││ Cache  710k ││ prediction   ││                    │
│              ││ ▁▃▅▇█▇▅▃▁▃▅││  feat/x +1~2 ││ ORPHAN PORTS       │
│ CODEX        ││ Turns: 48   ││              ││ :4000 old-prj node│
│ 5h █ 9%     ││ Avg: 25k/t  ││ api-server   ││                    │
│ 7d ██ 14%    ││             ││  main ✓clean ││                    │
└──────────────┘└─────────────┘└──────────────┘└────────────────────┘
┌─ ⁵sessions ─────────────────────────────────────────────────────────┐
│ ►*CC 7336 abtop  ● Exec opus  82% 1.2M  48  Edit src/pay.rs       │
│  >CD 8840 pred   ◌ Wait sonn  91% 340k  12  waiting                │
│ ─────────────────────────────────────────────────────────────────── │
│  SESSION 7336 · /Users/graykode/abtop                               │
│  Stripe payment integration...                                      │
│  └─ Edit src/pay.rs                                                 │
│  CHILDREN: 7401 cargo build                                         │
│  SUBAGENTS: explore-data ✓12k · run-tests ●8k                      │
│  MEM 4f · 12/200 │ v2.1.86 · 47m                                   │
└──────────────────────────────────────────────────────────────────────┘
```

Desktop rows (`desktop_layout`): every enabled pane reserves its floor before any pane grows, so a long session list cannot crowd out the top row.
1. **Header** (1 row) + **Footer** (1 row) — always present
2. **Floors** — mid-tier 6 rows (7 with Tokens), Sessions 5, Agents 3, top row 5. The top row is the only floor that yields, and only when the others leave fewer than 5 rows (below 22 rows with every pane enabled)
3. **Growth**, in order — Sessions to ideal (2/session + 7, min 8), Agents to 7, mid-tier to 8, top row to its ideal (sessions + 4, clamped to 5–10, minimum 6 with the cache chart); Sessions absorbs any remainder
4. **Widths** — mid-tier panes (quota, tokens, models, projects, ports, MCP) reserve minimum widths, then share surplus equally; switch to tabs when enabled panes cannot fit (134 columns with all enabled). Token rate and Context share the top row via `graph_band`: Context takes 35% but never less than its full table (34 columns), or either takes the full width alone

The narrow Usage tab places Token rate and Context side by side in one `graph_band`, then Quota, Tokens, and Models. Stats floors come first (Quota 5 without its summary, Tokens 7, Models 3); the graph band (5 rows) comes next and yields as a unit only when those floors leave no room (below 23 terminal rows); rows above the floors then go, in order, to Quota's summary, a second Models row, and a sixth band row when the cache-read chart is on; spare rows are shared evenly. None of this changes persisted visibility. Short quota panes show reset countdowns inline; bars and aggregate summaries yield before quota values. Mouse targeting and keyboard zoom use the visible layout.

Panel descriptions:
- **⁰agents**: Full-width row directly above Sessions; also first in the narrow Work tab. Shows a stacked Claude/Codex braille histogram (OMP and Pi stack in the Codex layer) using the Token rate renderer and theme gradients for concurrent working main agents and subagents (all nesting levels), current provider counts, peak, and a compact Unknown annotation. Each working main session contributes one, including sessions with no children; working descendants contribute independently. Waiting/rate-limited/background main sessions contribute zero themselves. `0` toggles persisted `show_agents` (default true); `g` toggles independent `agent_scope` (default `all_live`, alternative `selected`). History retains timestamped samples for 15 minutes while hidden and after sessions close, with no disk persistence/backfill. Each rendered bucket keeps the provider counts from one simultaneous peak sample. Reserve a three-row summary after essential pane minima, grow Sessions to ideal first, then grow the chart to seven rows. Claude lifecycle uses launch/completion records, child turns, workflow journal `started`/`result`/`failed`, and the run's `workflows/{runId}.json` completion record; Codex uses child metadata and own-turn start/end events, excluding inherited history. Quiet unfinished work stays active. Token/model accounting remains separate; JSON exposes child-only `subagent_counts` and combined `agent_counts` without history. OpenCode subagent tracking is unavailable.
- **¹context**: Project-labeled per-session context usage bars, percentages, window sizes, and compaction counts. `1` toggles persisted `show_context`.
- **⁹token rate**: Global token rate braille chart (200-point history) and tokens/min, counting input, output, and cache creation but excluding cache reads. Independent of selection and Tokens/Models stats scope. `9` toggles persisted `show_token_rate`. A missing setting inherits `show_context` for compatibility; fresh configurations enable both. The narrow Usage tab places Token rate beside Context, above Quota, Tokens, and Models. The optional cache-read chart (`C`, persisted `show_cache_read_rate`, default false) appears below the original chart with independent scaling and the theme's cache gradient. Both charts use synchronized per-refresh category deltas and 200-point histories; their numeric tokens/min summaries use timestamped activity from the preceding 60 seconds, retaining observed activity until expiry after sessions close. Reuse is `cache reads / (input + cache reads + cache writes) × 100`, excluding output; show `—` without input-side activity. Short panes retain a cache summary. Enabling it raises the top row's ideal minimum to six rows on desktop and in the narrow graph band, with five-row compact fallback; pane-level controls remain unchanged. It measures token reuse, not dollar savings.
- **²quota**: Claude + Codex rate limit gauges side-by-side (5h and 7d windows with reset countdown). Quota is intentionally limited to Claude and Codex; do not add an OpenCode row unless OpenCode exposes a reliable account-level provider rate-limit source.
- **³tokens**: Total token breakdown (in/out/cache) + per-turn sparkline for selected session.
- **Stats scope (`a`)**: Tokens and Models share `Selected` / `All live`, initially Selected and persisted as `stats_scope`. All live sums lifetime usage of unique, confirmed live sessions, including waiting and rate-limited sessions, independently of session search/selection. Done and Unknown sessions are excluded; totals drop when sessions close. Existing collector subagent rollups are counted once. The aggregate Tokens sparkline uses rolling tokens/min across Input + Output + CacheW, sampled each refresh (200 points); Selected keeps per-turn history. The header control is clickable with `--mouse`, and the View menu exposes the same toggle. Quota, Context, JSON snapshots, and `--once` retain their existing scope.
- **⁸models**: Lifetime token totals, percentage share, and rolling 60-second token rates per model in the selected statistics scope. Rates follow the percentage and sum Input + Output + CacheW, excluding CacheR; startup usage establishes a baseline. Enabled by default beside Tokens and in the narrow-terminal Usage tab; `8` toggles persisted `show_models`. Includes Claude's tracked subagents and nested workflow agents; Codex and OpenCode retain their existing per-session collector accounting. Model totals match the Tokens pane: Input + Output + CacheW, excluding CacheR. Missing attribution uses `Unknown`; overflow uses `Other (N)` with summed rates. Narrow entries keep metrics on their second line, wrapping the rate to a third only when needed, without increasing minimum pane width. JSON snapshots expose the complete `model_usage` lifetime list; rate bookkeeping is not serialized.
- **projects** (always visible): Per-project git branch + added/modified file counts.
- **⁴ports**: Agent-spawned open ports + orphan ports (from dead sessions). Conflict detection.
- **⁵sessions**: Full-width panel below mid row. Session list table (top) + selected session detail (bottom), separated by divider.

## Data Sources

All read-only from local filesystem + `ps` + `lsof`. No API calls, no auth.

### 1. Claude Code session discovery: process + config-root mapping

Discovery strategy:
1. Find running `claude` processes via `ps`
2. Map PID → open files/directories via `lsof`
3. Infer Claude config roots from open paths that contain `sessions/` and `projects/`
4. Read `{config-root}/sessions/{PID}.json`, falling back to scanning session files for the matching embedded PID
5. Parse `{config-root}/projects/{encoded-path}/{sessionId}.jsonl`

Fallback config roots are still scanned: `~/.claude`, direct home profile roots matching `~/.claude-*` when they contain both `sessions/` and `projects/`, `claude_config_dirs` from `~/.config/abtop/config.toml`, abtop's own `CLAUDE_CONFIG_DIR`, and on Linux any `CLAUDE_CONFIG_DIR` read from `/proc/{pid}/environ`.

Session file format:
```json
{ "pid": 7336, "sessionId": "2f029acc-...", "cwd": "/Users/graykode/abtop", "startedAt": 1774715116826, "kind": "interactive", "entrypoint": "cli" }
```
- ~170 bytes. Created on start, deleted on exit.
- Verify PID alive with shared `ps` data containing a `claude` binary.
- Skip sessions whose PID descends from abtop's own `claude --print` summary children without hiding user-spawned non-interactive sessions.

### 2. Claude Code transcript: `{config-root}/projects/{encoded-path}/{sessionId}.jsonl`
Path encoding: `/Users/foo/bar` → `-Users-foo-bar`

Key line types:

**`assistant`** (tokens, model, tools):
```json
{
  "type": "assistant",
  "timestamp": "2026-03-28T15:25:55.123Z",
  "message": {
    "model": "claude-opus-4-6",
    "stop_reason": "end_turn",
    "usage": {
      "input_tokens": 2,
      "output_tokens": 5,
      "cache_read_input_tokens": 11313,
      "cache_creation_input_tokens": 4350
    },
    "content": [
      { "type": "text", "text": "..." },
      { "type": "tool_use", "name": "Edit", "input": { "file_path": "src/main.rs", ... } }
    ]
  }
}
```

**`user`** (prompts, version):
```json
{ "type": "user", "timestamp": "...", "version": "2.1.86", "gitBranch": "main", "message": { "role": "user", "content": "..." } }
```

**`last-prompt`** (session tail marker):
```json
{ "type": "last-prompt", "lastPrompt": "...", "sessionId": "..." }
```

- **Size: 1KB–18MB**. Append-only, new line per message.
- **Reading strategy**: On first discovery, scan full file to build cumulative token totals. Then watch file size — on growth, read only new bytes appended since last read (track file offset). This gives both lifetime totals and real-time updates without re-reading.
- **Partial line handling**: new bytes may end mid-JSON-line. Buffer incomplete lines until next read.
- **File rotation**: if file shrinks (session restart), reset offset to 0 and re-scan.

### 3. Codex CLI sessions: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`

Discovery strategy:
1. Find running `codex` processes via `ps`
2. Map PID → open `rollout-*.jsonl` file via `lsof`
3. Parse JSONL for `session_meta`, `token_count` (includes rate_limits), `agent_message` events
4. Detect finished sessions: scan today's directory for JSONL < 5 min old not owned by running process

Rate limits extracted from `token_count` events:
```json
{
  "rate_limits": {
    "limit_id": "codex",
    "primary": { "used_percent": 9.0, "window_minutes": 300, "resets_at": 1774686045 },
    "secondary": { "used_percent": 14.0, "window_minutes": 10080, "resets_at": 1775186466 },
    "plan_type": "plus"
  }
}
```

### 4. OpenCode sessions: `~/.local/share/opencode/opencode.db`
- Discover running `opencode` processes via shared `ps` data.
- Read recent sessions from OpenCode's SQLite DB through `sqlite3 -readonly -json`.
- Match live PIDs to DB sessions by process cwd. OpenCode does not expose a PID/session mapping, so when multiple DB rows share one cwd, only live PIDs should be assigned and older rows should not be shown as live duplicates.
- OpenCode contributes session/token/project/port data, but not quota data. Quota remains Claude + Codex only.

### 5. OMP and Pi sessions: `~/.omp/agent/sessions/`, `~/.pi/agent/sessions/`
- One collector (`PiCollector`) serves both; `OMP_SPEC` / `PI_SPEC` hold the binary name, dot dir, and env vars.
- Discover `omp` / `pi` processes via shared `ps` data (`cmd_matches_harness`, which also accepts Pi's npm package entry). Drop a launcher wrapper when its child is also a candidate; skip `__omp_worker_*` and Pi's non-interactive subcommands.
- Agent dirs: `~/.{omp,pi}/agent`, `{OMP,PI}_CODING_AGENT_DIR`, the parent of `{OMP,PI}_CODING_AGENT_SESSION_DIR` (read from abtop's environment), plus any `<dir>/sessions/` ancestor of an open file.
- Map PID → transcript, first match wins: open `sessions/<project>/<file>.jsonl`; open lock file (`run/session-owners/<sid>.lock` or `.<file>.jsonl.owner.lock`); `--session` / `--resume` argument; `terminal-sessions/<tty>`; newest unclaimed `.jsonl` in the cwd's session folder. The last step is a guess and can pick another session's file when several share a cwd.
- Transcript lines: `session` (id, cwd, timestamp), `title` / `title_change`, `model_change`, `thinking_level_change` (effort), `message` with `usage` (`input`, `output`, `cacheRead`, `cacheWrite`) and `contextSnapshot`. Subagents are JSONL files under the folder named after the transcript, minus `.jsonl`.
- Context % uses `context_window_for_model` (Claude sizes), so it is approximate for non-Claude models. No quota and no memory status.

### 6. Subagents and workflows: `~/.claude/projects/{path}/{sessionId}/`
- `subagents/agent-{hash}.jsonl` — same JSONL format as main transcript
- `subagents/agent-{hash}.meta.json` — `{ "agentType": "general-purpose", "description": "..." }`
- `subagents/workflows/{runId}/agent-{hash}.{jsonl,meta.json}` — agents of a Workflow run (`runId` like `wf_f13c2bf8-4a9`), same format
- `workflows/scripts/{name}-{runId}.js` — the run's script; abtop takes the workflow name from this filename
- `workflows/{runId}.json` — completion record (`status`, `durationMs`, ...), written only when the run ends. Absent while running.

Claude Code also records pending background work on the main transcript's
`turn_duration` line, written at the end of every turn:
```json
{ "type": "system", "subtype": "turn_duration", "durationMs": 463584, "pendingWorkflowCount": 1, "pendingBackgroundAgentCount": 1 }
```
Each `pending*` key is omitted when its count is 0, so the latest `turn_duration` line is the current snapshot. Background Bash (`run_in_background`) and Monitor tasks are **not** counted there.

Full on-disk reference for background work (launch results, task notifications, completion records, signals that look useful but aren't): [docs/claude-code-background-work.md](./docs/claude-code-background-work.md).

### 7. Process tree: `ps` + `lsof`
```bash
ps -eo pid,ppid,rss,%cpu,command    # All processes
lsof -i -P -n -sTCP:LISTEN         # Open ports
```
- Build parent→children map from ppid
- Map listening PID → parent agent PID → session

### 8. Git status per project
```bash
git -C {cwd} status --porcelain     # added/modified file counts
```

### 9. Memory status
- Path: `~/.claude/projects/{encoded-path}/memory/`
- Count files in directory + lines in `MEMORY.md`

### 10. Rate limit (Claude Code)

NOT in transcript JSONL. Collected via StatusLine mechanism.

`abtop --setup` automates this: creates a script at `~/.claude/abtop-statusline.sh` that writes rate limit JSON to `~/.claude/abtop-rate-limits.json`, and registers it in `~/.claude/settings.json`.

File format read by abtop:
```json
{
  "source": "claude",
  "five_hour": { "used_percentage": 35.0, "resets_at": 1774715000 },
  "seven_day": { "used_percentage": 12.0, "resets_at": 1775320000 },
  "updated_at": 1774714400
}
```
- Rejects stale data (> 10 minutes old).
- `rate_limits` only present for Pro/Max subscribers.
- Account-level metric, shared across all sessions.
- Show "—" when not configured or data unavailable.

### 11. Other files
- `~/.claude/stats-cache.json` — daily aggregates. Only updated on `/stats`, NOT real-time.
- `~/.claude/history.jsonl` — prompt history with sessionId.

## Session Status Detection

Claude Code (`src/collector/claude.rs`), checked in order, first match wins:

```
● Exec  = latest assistant turn left a tool_use unanswered
◉ Think = trailing transcript line is a real user prompt (no assistant reply yet)
◐ Bg    = turn ended, but background work is pending: the latest turn_duration
          reports pendingWorkflowCount / pendingBackgroundAgentCount > 0, or a
          subagent / workflow agent is still working (same lifecycle as ⁰agents)
● Exec  = a descendant process uses >= 5% CPU
◌ Wait  = none of the above
```

There is no transcript-mtime gate: Claude Code writes an assistant turn atomically, so mtime goes stale during long streamed replies. `◐ Bg` is checked before descendant CPU because a workflow agent's Bash calls are descendants of the session's pid. A Claude session whose pid is gone is dropped, not shown as Done. Codex and OpenCode have their own rules in their collectors and never emit `◐ Bg`. OMP and Pi (`pi.rs`) check a pending tool call (`● Exec`), model generation (`◉ Think`), a working subagent (`◐ Bg`), then descendant and own CPU. `⏳ Rate` is applied afterwards (`app.rs`) to `Waiting` sessions whose agent is above 90% rate-limit usage.

**PID reuse risk**: verify PID is still the expected agent process (Claude, Codex, OpenCode, OMP, or Pi) by checking `ps -p {pid} -o command=`. Don't trust PID alone.

Current task (2nd line under each session):
- Exec → last `tool_use` name + first arg (e.g. `Edit src/main.rs`)
- Think → "thinking..."
- Bg → `workflow {name} · {working agent}`, `workflow {name}`, `waiting on N workflows`, `background agent {name}` or `waiting on N agents`
- Wait → "waiting for input"

**Known limitations** (all heuristic):
- Cannot distinguish rate-limit-waiting vs permission-prompt
- Background Bash (`run_in_background`) and Monitor tasks are not tracked: a session waiting only on those shows `◌ Wait` (or `● Exec` while the process burns CPU)
- If background work dies without a follow-up turn, the last `turn_duration` counts can keep the session in `◐ Bg` until the next turn
- Status is best-effort, not authoritative

## Session Summary Generation

Each session gets a one-line summary title generated via `claude --print`:
- Spawned as background process with 10s timeout
- Rejects generic/empty output; falls back to sanitized first prompt (28 chars)
- Cached to `~/.cache/abtop/summaries.json` (persists across runs)
- Max 3 concurrent summary jobs, max 2 retries per session

## Token Accounting

Cumulative totals and derived usage use input + output + cache writes, excluding
cache reads. Use the shared `token_total` helper and `AgentSession::total_tokens()`.
Preserve raw cache-read counters separately; the CacheR row is explicitly excluded
from Total. Category bars use the sum of all four raw categories. Context
occupancy and account-level quota calculations retain their independent meanings.

## Context Window Calculation

Not provided in data files. Derive:
- **Window size**: hardcode by model name
  - `claude-opus-4-6` → 200,000 (default)
  - `claude-opus-4-6[1m]` → 1,000,000
  - `claude-sonnet-4-6` → 200,000
  - `claude-haiku-4-5` → 200,000
- **Current usage**: last `assistant` line's `input_tokens + cache_read_input_tokens`. `cache_creation_input_tokens` is intentionally excluded — on compaction turns the same tokens can be reported as both `cache_creation` *and* `cache_read`, and summing all three double-counts (#54). Matches Claude Code's own statusline and the Codex collector.
- **Percentage**: current_usage / window_size * 100
- **Warning**: yellow at 80%, red at 90%, ⚠ icon at 90%+

## Orphan Port Detection

Tracks child processes that have open ports. When a parent session dies but the child process remains alive and listening:
- Added to `orphan_ports` list automatically
- Displayed in ports panel under "ORPHAN PORTS" section
- Can be killed via `X` (Shift+X) with safety checks (fresh port scan + PID command verification before SIGKILL)

## Key Bindings

| Key | Action |
|-----|--------|
| `↑`/`↓` or `k`/`j` | Select session in list |
| `Enter` | Jump to session terminal (cmux / tmux / iTerm2) |
| `x` | Kill selected session (SIGKILL) |
| `X` | Kill all orphan ports |
| `q` | Quit |
| `r` | Force refresh |
| `1` | Toggle Context pane |
| `8` | Toggle Models pane |
| `9` | Toggle Token rate pane |
| `C` (Shift+C) | Toggle cache-read chart and reuse inside Token rate |
| `a` | Toggle Tokens/Models scope: selected / all live |

## Tech Stack

- **Rust** (2021 edition)
- **ratatui** + **crossterm** for TUI
- **serde** + **serde_json** for JSON/JSONL parsing
- **chrono** for timestamp formatting
- **dirs** for home directory resolution
- **Polling intervals** (staggered to avoid freezes):
  - Session scan + transcript tail: every 2s
  - Process tree (ps): every 2s
  - Port scan (lsof) + git status + rate limits: every 10s (5 ticks)

## Commit Convention

```
<type>: <description>
```
Types: `feat`, `fix`, `refactor`, `docs`, `chore`

## Commands

```bash
cargo build                    # Build
cargo run                      # Run TUI
cargo run -- --once            # Print snapshot and exit
cargo run -- --setup           # Install StatusLine hook for rate limit collection
cargo run -- --exit-on-jump    # Quit after Enter-jumping to a session terminal (for popup overlays)
cargo test                     # Tests
cargo clippy                   # Lint
```

## Release Process

1. Pick the target semver version and update both `Cargo.toml` and `Cargo.lock`.
2. Verify the package locally:
   ```bash
   cargo test
   cargo clippy -- -D warnings
   cargo build --release
   cargo publish --dry-run
   ```
3. Commit and merge or push the version bump to `main`:
   ```bash
   git add Cargo.toml Cargo.lock
   git commit -m "chore: bump version to X.Y.Z"
   git push origin main
   ```
4. From a clean, up-to-date `main`, create and push an annotated release tag:
   ```bash
   git tag -a vX.Y.Z -m "vX.Y.Z"
   git push origin vX.Y.Z
   ```
5. Watch the tag-triggered workflows:
   ```bash
   gh run list --workflow Release --limit 5
   gh run list --workflow "Publish to crates.io" --limit 5
   ```
6. `release.yml` builds platform binaries, creates the GitHub Release, and updates the Homebrew formula.
7. `publish.yml` runs `cargo publish` to crates.io automatically.

**Do NOT run `cargo publish` or `gh release create` manually** — the CI workflows handle both.
**Do NOT push the tag before the version bump is on `main`.**
**Do NOT reuse a release tag after a failed publish; bump to a new patch version instead.**

## Non-Goals (v0.1)

- Gemini/Cursor support
- Cost estimation
- Remote/SSH monitoring
- Notifications/alerts

## Terminal Jump (`Enter`)

`Enter` focuses the terminal running the selected session's agent process.
The logic lives in `src/jump/` as a registry of `TerminalJumper` adapters
(one file per backend). `jumpers()` is the single ordered source of truth;
`resolve()` walks it and the first applicable adapter wins.

Each adapter returns a three-way `JumpAttempt`:
- `NotApplicable` — not this backend's terminal; try the next adapter.
- `Jumped` — focused successfully; stop.
- `Failed(msg)` — this backend owns the process but the focus command errored;
  stop and surface `"<backend>: <msg>"` in the status line.

Order (most specific first), mutually exclusive by controlling tty:

1. **cmux** (`jump/cmux.rs`) — reads `CMUX_WORKSPACE_ID` (a UUID cmux exports
   into every surface, inherited by the agent) from the process environment via
   `ps eww`, then `cmux select-workspace --workspace <uuid>`.
2. **tmux** (`jump/tmux.rs`) — only when abtop itself runs inside tmux (`$TMUX`).
   Maps PID → pane via `tmux list-panes -a -F '#{pane_pid} #{session_name}:#{window_index}.#{pane_index}'`
   + process-tree descent, then `switch-client` / `select-window` / `select-pane`.
   PID in no pane → `NotApplicable` (lets another backend try).
3. **iTerm2** (`jump/iterm2.rs`) — resolves the PID's controlling tty (`ps -o tty=`),
   then AppleScript selects the session whose `tty` matches and brings its
   window/app to the front. First call triggers a one-time macOS Automation
   permission prompt; until granted, `osascript` exits non-zero → `Failed`.

Parsing/registry logic is unit-tested in `jump/mod.rs`; the thin `ps`/`osascript`/
`tmux` I/O wrappers are verified manually.

## Privacy

abtop reads transcripts, prompts, tool inputs, and memory files. These may contain secrets.
- **`--once` output**: redact file contents from tool_use inputs. Show tool name + file path only, not content.
- **TUI mode**: show tool name + first arg (file path), never show file contents or prompt text in session list.
- **No network**: abtop never sends data anywhere. All local reads.
- **Exception**: summary generation calls `claude --print` locally (no network by abtop itself, but claude may use its API).

## Gotchas

- **Transcript size**: 1KB–18MB. On first load, full scan for totals. After that, track file offset and read only new bytes. Buffer partial lines.
- **Session file deletion**: files disappear when Claude exits. Handle `NotFound` between scan and read.
- **stats-cache.json is stale**: only updated on `/stats` command. Don't use for live data.
- **Context window not in data**: must hardcode per model. Will break if Anthropic/OpenAI add new models.
- **Rate limit is account-level**: shared across all sessions. Don't show per-session.
- **Path encoding**: `/Users/foo/bar` → `-Users-foo-bar`. Used for transcript directory names.
- **Path encoding collision**: `-Users-foo-bar-baz` could be `/Users/foo/bar-baz` or `/Users/foo-bar/baz`. Use session JSON's `cwd` as source of truth.
- **lsof can be slow**: on macOS with many open files. Cache results, poll every 10s.
- **Child process tree**: `pgrep -P` only gets direct children. Build full tree from `ps -eo ppid`.
- **Port detection race**: a port can close between lsof and display. Show stale data gracefully.
- **Subagent directory may not exist**: only created when Agent tool is used. Check existence before scanning.
- **Undocumented internals**: all data sources are Claude Code/Codex implementation details, not stable APIs. Schema may change without notice. Defensive parsing with `serde(default)` everywhere.
- **Terminal size**: minimum 80x24. Panels degrade gracefully when small: panes shrink to their floors first, and the Token rate / Context graphs yield only when those floors fill the height (desktop and narrow Usage tab).
- **PID reuse in port cache**: invalidate cached ports when the set of tracked PIDs changes.
- **Rate limit staleness**: reject rate limit data older than 10 minutes.
- **`/clear` + multi-PID same cwd**: after `/clear`, Claude Code mints a new `sessionId` + `.jsonl` without rewriting `sessions/{PID}.json`. abtop overrides the stale sid by picking the newest transcript in the project dir, but this heuristic can't disambiguate ownership when two live `claude` PIDs share a cwd — so the override is disabled in that case and both sessions keep their original sid until exit. Use separate worktrees if live tracking is needed on both simultaneously.
