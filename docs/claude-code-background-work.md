# Claude Code background work: on-disk signals

How an external monitor can tell **"turn ended, background work still running"** apart from
**"idle, waiting for the user"** in a Claude Code session. This backs abtop's `◐ Bg` status.

> **Provenance.** These formats were observed on disk on 2026-10-14, with Claude Code 2.1.299 on Linux.
> They are not a documented contract. Every key below was confirmed in real transcripts, but
> Claude Code can change them without notice. Re-verify against a live session before relying on
> a detail that isn't listed under *What abtop uses*.

Placeholders: `<root>` is the config root (e.g. `~/.claude`), `<enc>` is the encoded cwd
(`/a/b` → `-a-b`), and `<S>` is the session id.

## What abtop uses

| Signal | Where | Meaning |
|---|---|---|
| `turn_duration` pending counts | main transcript | Authoritative count of pending workflows and async agents at the end of each turn |
| Workflow agent transcripts | `<root>/projects/<enc>/<S>/subagents/workflows/<runId>/` | Which agent is working right now (lifecycle from child turns and `journal.jsonl`, shared with the ⁰agents pane) |
| Workflow script filename | `<root>/projects/<enc>/<S>/workflows/scripts/<name>-<runId>.js` | Human-readable workflow name |
| Completion record | `<root>/projects/<enc>/<S>/workflows/<runId>.json` | The run has ended, so its agents are done |

## `turn_duration`: the authoritative pending count

Every turn ends with a `system` line in the main transcript `<root>/projects/<enc>/<S>.jsonl`:

```json
{ "type": "system", "subtype": "turn_duration", "durationMs": 463584, "messageCount": 152,
  "pendingWorkflowCount": 1, "pendingBackgroundAgentCount": 1, "isMeta": false, "isSidechain": false }
```

- `pendingWorkflowCount` and `pendingBackgroundAgentCount` are **omitted when zero**. The latest
  `turn_duration` line is therefore the current snapshot, and a later line without the keys means nothing is pending.
- When the work reports back, Claude Code starts a new turn. That turn ends with a fresh `turn_duration`,
  so the counts clear on their own.
- Background Bash (`run_in_background`) and Monitor tasks are **not** counted here, and no other
  `pending*` keys exist.
- Metadata lines (`last-prompt`, `mode`, `permission-mode`, ...) can follow `turn_duration`.
  An incremental reader must keep the previous counts across a delta that has no `turn_duration`.

The assistant line just before it has `stop_reason: "end_turn"`. A session waiting on a workflow
therefore looks exactly like an idle one unless you read these counts.

## Workflow run layout

Launching a workflow with the `Workflow` tool creates:

```
<root>/projects/<enc>/<S>/
├── workflows/
│   ├── scripts/<name>-<runId>.js        # script, written at launch
│   └── <runId>.json                     # completion record, written ONLY when the run ends
└── subagents/workflows/<runId>/
    ├── journal.jsonl                    # {"type":"launched"}, {"type":"started","agentId","label","phase",...}
    ├── agent-<id>.meta.json             # agentType "workflow-subagent", description, workflowPhase, spawnDepth, model
    └── agent-<id>.jsonl                 # normal sidechain transcript (isSidechain: true)
```

- `runId` looks like `wf_f13c2bf8-4a9`. It is distinct from the task id (e.g. `w81b8t24o`).
- A running workflow has **no** status field anywhere. It is running when the run dir exists, there is no `<runId>.json`,
  and agent transcripts are still being appended. A crashed run never writes `<runId>.json`, so stale run dirs
  without one exist. Only trust them while `pendingWorkflowCount > 0`.
- `journal.jsonl` holds only launch and start events. No completion line was observed.
- Completion record keys: `runId`, `timestamp`, `taskId`, `script`, `scriptPath`, `result`, `agentCount`,
  `logs`, `durationMs`, `summary`, `workflowName`, `status` (`"completed"`), `startTime`, `phases`,
  `defaultModel`, `workflowProgress`, `totalTokens`, `totalToolCalls`.

Plain async Agent subagents sit one level up, as `subagents/agent-<id>.{jsonl,meta.json}`.

## Launch results in the main transcript

The `tool_result` of each background launch is a plain string:

| Kind | `tool_result` text |
|---|---|
| Workflow | `Workflow launched in background. Task ID: <taskId>` … `Transcript dir: <S>/subagents/workflows/<runId>` … `Run ID: <runId>` |
| Bash | `Command running in background with ID: <id>. Output is being written to: /tmp/claude-<uid>/<enc>/<S>/tasks/<id>.output` |
| Monitor | `Monitor started (task <id>, expires in 30m ...)` |
| Agent | `Async agent launched successfully. (...)` |

Every background task has an output file at `/tmp/claude-<uid>/<enc>/<S>/tasks/<taskId>.output`.

## Completion: task notifications

When a background task finishes, the main transcript gets:

1. `{"type":"queue-operation","operation":"enqueue","timestamp":...,"content":"<task-notification>..."}`,
   later followed by `{"type":"queue-operation","operation":"dequeue",...}`. Neither line has a `sessionId` or `uuid`.
2. A `user` line with `origin: {kind: "task-notification", producer: "session-task"}`,
   `promptSource: "system"` and `turnOrigin: "task_notification"`, whose content is the same notification.

```
<task-notification>
<task-id>w7p8b0h7e</task-id>
<tool-use-id>toolu_...</tool-use-id>
<output-file>/tmp/claude-<uid>/<enc>/<S>/tasks/w7p8b0h7e.output</output-file>
<status>completed</status>
<summary>Dynamic workflow "..." completed</summary>
<result>...</result>
</task-notification>
```

- Observed `<status>` values: `completed`, `failed`, `killed`, `stopped`, `starting`.
- Monitor events use the same envelope with `<summary>Monitor event: "..."</summary>` and `<event>`, and no `<status>`.
- abtop's parser treats this `user` line as a real prompt, so the session goes to Think while the model
  handles the result. That is the correct behaviour.

## Signals that look useful but aren't

- **`<root>/sessions/<pid>.json` `status` (`busy` / `idle`).** It stayed `busy` for the whole wait after `end_turn`,
  so it cannot separate "model generating" from "waiting on background work". The file also outlives dead pids.
- **`<root>/jobs/<id>/state.json` `inFlight`.** It exists only for daemon-managed sessions, and its behaviour during a workflow was not verified.
- **Main transcript mtime.** It goes quiet for the whole wait, while workflow agent transcripts keep growing.

## Known gaps in abtop

- Background Bash and Monitor are not tracked. Doing so would mean pairing launch ids from `tool_result`
  with terminal `<task-notification>` statuses, since `turn_duration` doesn't count them.
- If background work dies without a follow-up turn (e.g. an interrupt), the last `turn_duration` counts can
  keep a session in `◐ Bg` until the next turn. This has not been observed yet.
