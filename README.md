# abtop

**Like [btop](https://github.com/aristocratos/btop), but for your AI coding agents.**

See every Claude Code, Codex CLI, and OpenCode session at a glance — token usage, context window %, rate limits, child processes, open ports, and more.
Claude Code, Codex CLI, and OpenCode sessions are discovered from local process/file state, so multiple active profiles are supported across macOS, Linux, and Windows.

![demo](https://raw.githubusercontent.com/graykode/abtop/main/assets/demo.gif)

## Token accounting

Token totals use **input + output + cache writes**, excluding cache reads. This
basis applies to session and subagent totals, Tokens averages and per-turn
history, activity rates, Models attribution and shares, All live statistics, and
plain/JSON snapshots. Cache writes remain included.

The CacheR row is labeled `excluded` (or `excl` in compact layouts) because it is
excluded from Total. It remains a separate raw counter, including when Total is
zero; Models shows an explicit empty state when no qualifying usage exists.
Category bars show each category's share of **all four raw categories**, including cache reads; they are
not shares of the headline Total. Cached context still occupies the context
window, and account quotas retain their provider-defined meanings.

Excluding cache reads focuses headline totals on token categories that generally
cost more per token, making spending drivers easier to gauge. These totals are
not cost estimates: prices vary by model and category, and large cache-read
volumes can still contribute materially to the bill. Cache reads remain visible
separately.

For library consumers,
`AgentSession::total_tokens()` is the single total API; `active_tokens()` has been
removed. Snapshot field names and raw category counters are unchanged.

## Why

- Running 3+ agents across projects? See them all in one screen.
- Hitting rate limits? Watch your quota in real-time.
- Agent spawned a server and forgot to kill it? Orphan port detection.
- Context window filling up? Per-session % bars with warnings.

All read-only. No API keys. No auth.

## Install

### macOS / Linux

> [!IMPORTANT]
> On Linux, ensure `sqlite3` is installed to enable monitoring for OpenCode sessions.

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/graykode/abtop/releases/latest/download/abtop-installer.sh | sh
```

### Cargo

```bash
cargo install abtop
```

### Windows

Native support — no WSL required. Uses `sysinfo` for process info and host CPU/MEM metrics, and `netstat -ano` for listening ports. Windows has no load average, so LOAD is reported as 0. OpenCode session discovery additionally requires the `sqlite3` CLI (`winget install SQLite.SQLite`); without it abtop prints a one-time warning to stderr.

```powershell
powershell -c "irm https://github.com/graykode/abtop/releases/latest/download/abtop-installer.ps1 | iex"
```

Or `cargo install abtop` from any terminal with Git in PATH. Claude Code config is resolved automatically from `%USERPROFILE%\.claude`.

### Other

Pre-built binaries for all platforms are available on the [GitHub Releases](https://github.com/graykode/abtop/releases) page.

## Usage

```bash
abtop                    # Launch TUI
abtop --once             # Print snapshot and exit
abtop --json             # Print one JSON snapshot and exit (for scripts/tools)
abtop --setup            # Install rate limit collection hook
abtop --theme dracula    # Launch with a specific theme
abtop --mouse            # Enable mouse click/scroll navigation
```

Recommended terminal size: **120x40** or larger. Minimum 80x24 — panels hide gracefully when small.
Mouse capture is off by default so terminal drag selection and copy keep working. Launch with `--mouse` if you prefer click targets and wheel navigation.

The tokens pane shows the selected session's Input, Output, CacheR, and CacheW totals, followed by tokens per minute over a rolling 60-second window. Claude totals and rates include subagents, including nested workflow agents. Rates count increases observed while abtop runs, start at zero, and return to zero after a minute without new tokens. On narrow panes, meter bars shrink first; rates are hidden if the text still cannot fit.

The models pane shows lifetime token totals, percentage share, and tokens per minute for each model in the selected statistics scope, sorted by lifetime usage. The rate follows the percentage and sums Input + Output + CacheW, excluding CacheR, over a rolling 60-second window. Rates start at zero and count increases observed while abtop runs. It appears beside Tokens on wide terminals and in the Usage tab on narrow terminals. Press `8` to toggle it, or use the View/config menus; visibility is saved. Narrow panes use two-line entries, moving the rate to a third line only when the metrics do not fit. Overflow is combined into `Other (N)`, including its summed rate.

Press `a` to switch Tokens and Models together between **Selected** and **All live**. The header shows the current scope and live-session count; with `--mouse`, click that control to switch. The same action is available in the View menu (`v`). The initial scope is Selected, and your last choice is saved across restarts.

All live adds the lifetime usage of all confirmed live sessions from enabled collectors, including sessions waiting for input or rate limits. Finished sessions and sessions whose process ownership is unknown are excluded. Totals decrease when a session closes. Searching and selecting sessions only affects session navigation and details; it does not change All live totals. Quota remains account-level, and Context keeps its existing cross-session activity and per-session percentages.

Aggregate model shares use the combined token total, and average tokens per turn is the combined total divided by the combined turn count. The aggregate Tokens sparkline shows combined rolling `tokens/min`, excluding cache reads, sampled on each refresh for the sessions live at that time (up to 200 samples). History is collected even in Selected mode; discovering a session does not turn its past usage into a rate spike. Existing collector accounting is preserved: Claude session totals already include tracked subagents, while Codex and OpenCode totals cover each session itself.

The top row has two independent panes: **Token rate** (`9`) shows the global token activity chart and tokens/min; **Context** (`1`) shows each session's project name, context usage, window size, and compaction count. Token rate counts input, output, and cache creation across collected sessions, excluding cache reads. It stays global regardless of selection or the Tokens/Models scope toggle. On desktop, the panes share the row at 65%/35%; hiding either lets the other use the full width. Both toggles are available in the View menu and configuration overlay and persist across restarts.

Press **`C` (Shift+C)** to add an optional **Cache reads/min** chart below the existing Token rate chart. It is disabled by default and can also be toggled in the View menu or configuration overlay. The two charts use independent vertical scales, so compare their numeric rates rather than their heights. Both plot synchronized per-refresh activity and retain up to 200 points, including while hidden. Short panes keep a cache-rate and reuse summary when the second chart cannot fit; `9` still controls the whole pane.

Both rate labels use tokens observed during the preceding 60 seconds, based on elapsed time rather than a fixed number of refreshes. Startup usage is not extrapolated, and already-observed activity remains in the window until it expires even after a session closes. **Reuse %** is `100 × cache reads / (input + cache reads + cache writes)` over that same window; output is excluded, and `—` means there were no input-side tokens. This measures the share of reported input-side tokens served from cache. It is not a percentage of money saved: prices depend on the provider, model, and billing arrangement, and abtop does not estimate dollar costs.

The **Agents** panel (`0`) sits above Sessions and charts the preceding 15 minutes of concurrent Claude Code and Codex agent work, including main sessions and subagents. Braille bars use the same rendering style as Token rate, with theme gradients distinguishing the two stacked providers; the legend shows current counts and the observed peak. Press `g` (or click the panel title with `--mouse`) to switch between **All live**, the default, and **Selected**. This scope is independent of Tokens/Models. Each actively working main session counts once, even without subagents. Its working descendants each count once as well: one working main session with two working subagents counts as three. Waiting and rate-limited main sessions do not contribute to the working count; their working children still do. Shell commands and workflow containers are excluded. The narrow Work tab supports the same panel and zoom controls. Short layouts retain its summary before hiding its chart.

A child counts as working during an unfinished work turn, including model generation and tool execution. Quiet periods do not automatically mark it done. Claude uses child transcripts, launch/completion records, and workflow journal starts, results, and failures; Codex uses child-thread relationships and turn start/completion/interruption events. A small **Unknown** count identifies incomplete lifecycle evidence. Discovery is best-effort and can lag a refresh; filesystem-only Codex children can take up to the 10-second discovery interval to appear. OpenCode subagent tracking is unavailable.

Agent history starts when abtop observes activity, continues while the pane is hidden, and remains in memory for 15 minutes. Closing a session lowers the current count without deleting its history. Each bar preserves a real simultaneous peak sample when the window is wider than the chart. History resets when abtop exits; it is not reconstructed from old transcripts. Token and model totals keep their existing accounting.

The desktop layout reserves minimum widths for enabled panes and switches to tabs when they cannot fit (134 columns with all panes enabled). In the Usage tab, Token rate hides first, then Context, when height is limited, preserving both quota windows and all four token categories. Quota bars shrink before values are clipped, reset countdowns move inline in shorter panes, and Models combines overflow when needed. These automatic layout changes do not alter saved visibility settings.

Model totals include input, output, and cache writes, excluding cache reads and matching the Tokens pane in either scope. Distinct model versions stay separate, and OpenCode entries include the provider. Missing model metadata is counted as `Unknown`. Codex attributes increases in cumulative usage to the current turn's model; if counters decrease, the replacement baseline becomes `Unknown` and subsequent increases resume model attribution.

### Terminal Jump

Press `Enter` to focus the terminal running the selected agent. abtop supports cmux, tmux, and iTerm2 on macOS.

```bash
tmux new -s work
# pane 0: abtop
# pane 1: claude (project A)
# pane 2: claude (project B)
# → Enter on a session in abtop jumps to its pane
```

## Supported Agents

| Feature           | Claude Code | Codex CLI | OpenCode |
| ----------------- | :---------: | :-------: | :------: |
| Session Discovery |     ✅      |    ✅     |    ✅    |
| Token Tracking    |     ✅      |    ✅     |    ✅    |
| Context Window %  |     ✅      |    ✅     |    ❌    |
| Status Detection  |     ✅      |    ✅     |    ✅    |
| Current Task      |     ✅      |    ✅     |    ❌    |
| Rate Limit        |     ✅      |    ✅     |    ❌    |
| Git Status        |     ✅      |    ✅     |    ✅    |
| Children / Ports  |     ✅      |    ✅     |    ✅    |
| Subagents         |     ✅      |    ❌     |    ❌    |
| Memory Status     |     ✅      |    ❌     |    ❌    |

OpenCode support reads the local SQLite database at `~/.local/share/opencode/opencode.db` (also the default location on Windows; `%LOCALAPPDATA%\opencode` and `%APPDATA%\opencode` are probed as fallbacks) and requires `sqlite3` in `PATH` (on Windows: `winget install SQLite.SQLite`).

## Themes

12 built-in themes, including 4 colorblind-friendly options (`high-contrast`, `protanopia`, `deuteranopia`, `tritanopia`). Press `t` to cycle at runtime, or launch with `--theme <name>`. Your choice is saved to `~/.config/abtop/config.toml`.

| btop (default) | dracula | catppuccin |
|:-:|:-:|:-:|
| ![btop](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/btop.png) | ![dracula](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/dracula.png) | ![catppuccin](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/catppuccin.png) |

| tokyo-night | gruvbox | nord |
|:-:|:-:|:-:|
| ![tokyo-night](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/tokyo-night.png) | ![gruvbox](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/gruvbox.png) | ![nord](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/nord.png) |

Colorblind-friendly themes:

| high-contrast | protanopia |
|:-:|:-:|
| ![high-contrast](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/high-contrast.png) | ![protanopia](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/protanopia.png) |

| deuteranopia | tritanopia |
|:-:|:-:|
| ![deuteranopia](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/deuteranopia.png) | ![tritanopia](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/tritanopia.png) |

Light themes (`light` — Solarized cream, `white` — GitHub-style pure white) for bright terminals:

| light | white |
|:-:|:-:|
| ![light](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/light.png) | ![white](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/white.png) |

## Configuration

`~/.config/abtop/config.toml` supports:

```toml
theme = "btop"
# Shared Tokens/Models scope: "selected" (default) or "all_live".
# Updated automatically when you press a or click the header scope control.
stats_scope = "selected"
# Show per-model usage for the current stats scope (enabled by default).
show_models = true
# Agent concurrency panel and its independent scope (0 / g).
show_agents = true
agent_scope = "all_live"
# Independent top-row panes (1: Context, 9: Token rate).
show_context = true
show_token_rate = true
# Optional cache chart and reuse metric inside Token rate (Shift+C).
show_cache_read_rate = false
# Hide specific agent CLIs from the TUI (case-insensitive).
# Useful if you only use one agent and want a cleaner view.
hidden_agents = ["codex"]
# Additional Claude Code profile roots to scan.
# abtop also auto-discovers ~/.claude and ~/.claude-* roots that contain
# both sessions/ and projects/.
claude_config_dirs = ["~/.claude-personal", "~/.claude-work-team"]
# UI language. Omit or leave empty to auto-detect from LANG.
language = "zh"
```

If `show_token_rate` is omitted, it inherits `show_context` (which defaults to `true`), preserving the old combined pane preference. Once saved, the two settings are independent.

### Supported Languages

| Code | Language            |
| ---- | ------------------- |
| `en` | English (default)   |
| `zh` | Simplified Chinese  |

When `language` is unset, abtop auto-detects from `LANG` — any value starting with `zh` switches to Simplified Chinese, otherwise English.

## Key Bindings

| Key                | Action                               |
| ------------------ | ------------------------------------ |
| `0`                | Toggle Agents panel               |
| `g`                | Agents: all live / selected        |
| `↑`/`↓` or `k`/`j` | Select session                       |
| `Enter`            | Jump to session terminal             |
| `x`                | Kill selected session                |
| `a`                | Toggle stats: selected / all live     |
| `X`                | Kill all orphan ports                |
| `t`                | Cycle theme                          |
| `1`–`8`            | Toggle panels (`8`: models)           |
| `Esc`              | Open/close config page               |
| `q`                | Quit                                 |
| `r`                | Force refresh                        |

## Library / JSON snapshot

abtop is also a library crate, so local tools can reuse its data-collection
layer in-process — no re-scanning, no subprocesses — and serialize the same
state the TUI renders.

```bash
abtop --json    # one-shot JSON snapshot for scripts
```

For long-running consumers, build an `App`, refresh it with
`App::tick_no_summaries()` (which never spawns `claude --print`, so it doesn't
touch your Claude quota), and call `App::to_snapshot(interval_ms)` to get a
JSON-serializable [`Snapshot`]:

```rust,no_run
use abtop::app::App;
use abtop::{config, theme::Theme};

let cfg = config::load_config();
let mut app = App::new_with_config_and_claude_dirs(
    Theme::default(), &cfg.hidden_agents, cfg.panels, &cfg.claude_config_dirs,
);
app.tick_no_summaries();
let json = serde_json::to_string(&app.to_snapshot(2_000)).unwrap();
```

`App` is not `Send` (it owns the collectors), so keep it on one thread and pass
the serialized JSON elsewhere. [abtop-web-ui](https://github.com/XKHoshizora/abtop-web-ui)
is a reference consumer: a local-first web dashboard built on exactly this API.

Each JSON session includes `model_usage`, an array of `{ "model": "...", "total_tokens": 123 }` entries sorted by usage. These entries use the same accounting and subagent scope as that session's `total_tokens`; percentage share is `100 * total_tokens / session.total_tokens` when the session total is nonzero. Library users constructing `AgentSession`, `SessionView`, or `PanelVisibility` literals must supply the new `model_usage` or `models` field as appropriate.

Stats scope affects the TUI only; `--once` and JSON snapshots retain their per-session output. Library users constructing `AppConfig` literals must supply `stats_scope` (or use `..Default::default()`). Existing `App` constructors default to `StatsScope::Selected`; set `app.stats_scope` explicitly to use another scope when rendering the TUI.

## Privacy

abtop reads local files and local process/open-file metadata only. No API keys, no auth. In the TUI and `--once` output, tool names and file paths are shown, but file contents and prompt text are never displayed. Session summaries are generated via `claude --print`, which makes its own API call — this is the only indirect network usage.

JSON snapshots add `subagent_counts` to each session (`working`, `unknown`) and to the top level (separate `claude` and `codex` counts). These remain child-only counts. The top-level `agent_counts` field includes working main sessions and their working children, matching the Agents chart. Both are complete current counts, independent of UI scope and the capped subagent detail list; history is not exported. A one-shot snapshot may precede background discovery of Codex children without open file descriptors.

The JSON snapshot includes richer local dashboard data, including `summary`, `chat_messages`, working directories, config roots, tool-call previews, child process commands, token counts, and port metadata. Chat text is bounded and redacted by the collectors, but it is still derived from local transcripts and may contain sensitive project context. Treat JSON snapshots as local/private data and avoid writing them to shared logs or exposing them on a network without your own access controls.

## Acknowledgements

Huge thanks to [@tbouquet](https://github.com/tbouquet) for driving much of abtop's recent shape — themes, config overlay and panel toggles, session filtering, subagent tree view, the context window gauge with compaction detection, plus a steady stream of fixes and security hardening along the way.

## License

MIT
