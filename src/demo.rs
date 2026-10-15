use crate::app::{App, AGENT_HISTORY_WINDOW};
use crate::model::{
    AgentSession, ChatMessage, ChatRole, ChildProcess, FileAccess, FileOp, LaunchSurface,
    ModelUsage, OrphanPort, RateLimitInfo, SessionStatus, SubAgent, ToolCall,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

pub fn populate_demo(app: &mut App) {
    let now = now_ms();

    // --- Sessions ---
    app.sessions = vec![
        AgentSession {
            agent_cli: "claude",
            launch_surface: LaunchSurface::Cli,
            pid: 7336,
            session_id: "a1b2c3d4-5678-9abc-def0-111111111111".into(),
            cwd: "/Users/demo/webshop".into(),
            project_name: "webshop".into(),
            started_at: now - 2 * 3600 * 1000, // 2h ago
            status: SessionStatus::Executing,
            model: "claude-opus-4-6".into(),
            effort: String::new(),
            context_percent: 72.0,
            total_input_tokens: 48_200,
            total_output_tokens: 12_800,
            total_cache_read: 1_420_000,
            total_cache_create: 185_000,
            model_usage: Vec::new(),
            turn_count: 34,
            current_tasks: vec!["Edit src/checkout/payment.rs".into()],
            mem_mb: 342,
            version: "2.1.87".into(),
            git_branch: "main".into(),
            git_added: 2,
            git_modified: 8,
            token_history: vec![
                18000, 22000, 45000, 38000, 52000, 41000, 35000, 28000, 61000, 55000, 48000, 39000,
                44000, 50000, 32000, 27000, 58000, 46000, 42000, 36000, 53000, 47000, 41000, 38000,
                62000, 55000, 49000, 43000, 51000, 44000, 38000, 33000, 56000, 48000,
            ],
            context_history: vec![
                20000, 35000, 52000, 68000, 85000, 102000, 118000, 135000, 148000, 162000, 175000,
                185000, 192000, // compaction event: 192k -> 65k (66% drop)
                65000, 78000, 92000, 108000, 125000, 138000, 145000, 155000, 168000, 178000,
                185000, 190000, // second compaction: 190k -> 58k
                58000, 72000, 88000, 105000, 120000, 135000, 142000, 148000,
            ],
            compaction_count: 2,
            context_window: 200_000,
            subagent_counts: Default::default(),
            subagents: vec![
                SubAgent {
                    activity_key: String::new(),
                    name: "Explore test coverage".into(),
                    status: "done".into(),
                    tokens: 12_400,
                    workflow: None,
                },
                SubAgent {
                    activity_key: String::new(),
                    name: "Run integration tests".into(),
                    status: "working".into(),
                    tokens: 8_200,
                    workflow: None,
                },
            ],
            pending_workflows: 0,
            pending_bg_agents: 0,
            mem_file_count: 4,
            mem_line_count: 12,
            children: vec![
                ChildProcess {
                    pid: 7401,
                    command: "cargo build --release".into(),
                    mem_kb: 342_000,
                    port: None,
                },
                ChildProcess {
                    pid: 7455,
                    command: "cargo test".into(),
                    mem_kb: 28_000,
                    port: None,
                },
            ],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Implement Stripe payment integration for checkout flow".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "I'll inspect checkout and config paths first, then wire the smallest payment boundary.".into(),
                },
                ChatMessage {
                    role: ChatRole::User,
                    text: "Keep webhook handling minimal and make tests cover declined cards.".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Payment code is in place; current pass is tightening test failures around webhook signatures.".into(),
                },
            ],
            initial_prompt: "Implement Stripe payment integration for checkout flow".into(),
            tool_calls: vec![
                ToolCall {
                    name: "Read".into(),
                    arg: "src/checkout/mod.rs".into(),
                    duration_ms: 85,
                },
                ToolCall {
                    name: "Read".into(),
                    arg: "Cargo.toml".into(),
                    duration_ms: 42,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo add stripe-rust".into(),
                    duration_ms: 3200,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/checkout/payment.rs".into(),
                    duration_ms: 120,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/checkout/mod.rs".into(),
                    duration_ms: 95,
                },
                ToolCall {
                    name: "Write".into(),
                    arg: "src/checkout/stripe.rs".into(),
                    duration_ms: 180,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo test".into(),
                    duration_ms: 8400,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/checkout/stripe.rs".into(),
                    duration_ms: 110,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo test checkout".into(),
                    duration_ms: 4200,
                },
                ToolCall {
                    name: "Read".into(),
                    arg: "src/config.rs".into(),
                    duration_ms: 55,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/config.rs".into(),
                    duration_ms: 90,
                },
                ToolCall {
                    name: "Grep".into(),
                    arg: "STRIPE_SECRET".into(),
                    duration_ms: 320,
                },
                ToolCall {
                    name: "Agent".into(),
                    arg: "security review".into(),
                    duration_ms: 12400,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo clippy".into(),
                    duration_ms: 5100,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/checkout/payment.rs".into(),
                    duration_ms: 145,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo test".into(),
                    duration_ms: 7800,
                },
                // Currently running: timeline bar grows in real time.
                ToolCall {
                    name: "WebSearch".into(),
                    arg: "stripe webhook best practice".into(),
                    duration_ms: 0,
                },
            ],
            pending_since_ms: now - 6_000, // 6s ago => bar animates
            thinking_since_ms: 0,
            config_root: "~/.claude".into(),
            file_accesses: vec![
                FileAccess {
                    path: "src/checkout/payment.rs".into(),
                    operation: FileOp::Read,
                    turn_index: 2,
                },
                FileAccess {
                    path: "src/checkout/mod.rs".into(),
                    operation: FileOp::Read,
                    turn_index: 3,
                },
                FileAccess {
                    path: "src/checkout/payment.rs".into(),
                    operation: FileOp::Edit,
                    turn_index: 5,
                },
                FileAccess {
                    path: "src/config/stripe.rs".into(),
                    operation: FileOp::Write,
                    turn_index: 7,
                },
                FileAccess {
                    path: "src/checkout/payment.rs".into(),
                    operation: FileOp::Edit,
                    turn_index: 10,
                },
                FileAccess {
                    path: "tests/checkout_test.rs".into(),
                    operation: FileOp::Write,
                    turn_index: 12,
                },
                FileAccess {
                    path: "src/models/order.rs".into(),
                    operation: FileOp::Read,
                    turn_index: 15,
                },
                FileAccess {
                    path: "src/checkout/payment.rs".into(),
                    operation: FileOp::Edit,
                    turn_index: 18,
                },
                FileAccess {
                    path: "Cargo.toml".into(),
                    operation: FileOp::Read,
                    turn_index: 20,
                },
                FileAccess {
                    path: "tests/checkout_test.rs".into(),
                    operation: FileOp::Edit,
                    turn_index: 22,
                },
            ],
        },
        AgentSession {
            agent_cli: "claude",
            launch_surface: LaunchSurface::App,
            pid: 8840,
            session_id: "b2c3d4e5-6789-abcd-ef01-222222222222".into(),
            cwd: "/Users/demo/ml-pipeline".into(),
            project_name: "ml-pipeline".into(),
            started_at: now - 47 * 60 * 1000, // 47m ago
            status: SessionStatus::Waiting,
            model: "claude-sonnet-4-6".into(),
            effort: String::new(),
            context_percent: 91.0,
            total_input_tokens: 82_000,
            total_output_tokens: 38_000,
            total_cache_read: 2_100_000,
            total_cache_create: 280_000,
            model_usage: Vec::new(),
            turn_count: 71,
            current_tasks: vec!["waiting for user input".into()],
            mem_mb: 128,
            version: "2.1.87".into(),
            git_branch: "feat/batch-inference".into(),
            git_added: 1,
            git_modified: 4,
            token_history: vec![
                32000, 28000, 41000, 55000, 62000, 48000, 35000, 29000, 44000, 58000, 51000, 39000,
                46000, 53000, 42000, 37000, 60000, 52000, 45000, 38000, 56000, 49000, 43000, 36000,
                63000, 57000, 50000, 44000, 54000, 47000,
            ],
            context_history: vec![
                15000, 28000, 45000, 62000, 80000, 95000, 112000, 128000, 142000, 158000, 172000,
                182000, 190000, 195000, // compaction: 195k -> 70k
                70000, 85000, 98000, 115000, 130000, 145000, 158000, 170000, 182000,
            ],
            compaction_count: 1,
            context_window: 200_000,
            subagent_counts: Default::default(),
            subagents: vec![],
            pending_workflows: 0,
            pending_bg_agents: 0,
            mem_file_count: 2,
            mem_line_count: 8,
            children: vec![],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Add batch inference endpoint with GPU scheduling".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Endpoint is implemented; waiting on your choice for queue priority behavior.".into(),
                },
            ],
            initial_prompt: "Add batch inference endpoint with GPU scheduling".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.claude-work".into(),
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "claude",
            launch_surface: LaunchSurface::Ide,
            pid: 9102,
            session_id: "c3d4e5f6-789a-bcde-f012-333333333333".into(),
            cwd: "/Users/demo/api-server".into(),
            project_name: "api-server".into(),
            started_at: now - 15 * 60 * 1000, // 15m ago
            status: SessionStatus::Executing,
            model: "claude-haiku-4-5".into(),
            effort: String::new(),
            context_percent: 42.0,
            total_input_tokens: 5_200,
            total_output_tokens: 2_800,
            total_cache_read: 320_000,
            total_cache_create: 45_000,
            model_usage: Vec::new(),
            turn_count: 12,
            current_tasks: vec!["Bash npm run dev".into()],
            mem_mb: 86,
            version: "2.1.87".into(),
            git_branch: "main".into(),
            git_added: 0,
            git_modified: 2,
            token_history: vec![
                8000, 12000, 15000, 22000, 18000, 25000, 20000, 16000, 28000, 24000, 19000, 14000,
            ],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagent_counts: Default::default(),
            subagents: vec![],
            pending_workflows: 0,
            pending_bg_agents: 0,
            mem_file_count: 1,
            mem_line_count: 3,
            children: vec![
                ChildProcess {
                    pid: 9150,
                    command: "node server.js".into(),
                    mem_kb: 98_000,
                    port: Some(3000),
                },
                ChildProcess {
                    pid: 9178,
                    command: "node worker.js".into(),
                    mem_kb: 52_000,
                    port: Some(3001),
                },
                ChildProcess {
                    pid: 9203,
                    command: "postgres -D /usr/local/var/postgres".into(),
                    mem_kb: 24_000,
                    port: Some(5432),
                },
            ],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Fix CORS headers and add rate limiting middleware".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "I'll patch middleware order, then run the API smoke tests.".into(),
                },
            ],
            initial_prompt: "Fix CORS headers and add rate limiting middleware".into(),
            tool_calls: vec![
                ToolCall {
                    name: "Read".into(),
                    arg: "src/server.ts".into(),
                    duration_ms: 60,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/middleware/cors.ts".into(),
                    duration_ms: 150,
                },
                ToolCall {
                    name: "Write".into(),
                    arg: "src/middleware/rate-limit.ts".into(),
                    duration_ms: 200,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "npm test".into(),
                    duration_ms: 2800,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/server.ts".into(),
                    duration_ms: 110,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "npm run dev".into(),
                    duration_ms: 1500,
                },
            ],
            pending_since_ms: 0,
            // Model is generating its next reply - virtual "Thinking" row
            // animates. 1s offset keeps the bar visibly growing against the
            // session's 2.8s max tool duration before it caps at 100%.
            thinking_since_ms: now - 1_000,
            config_root: "~/.claude".into(),
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "codex",
            launch_surface: LaunchSurface::Cli,
            pid: 8901,
            session_id: "d4e5f6a7-89ab-cdef-0123-444444444444".into(),
            cwd: "/Users/demo/data-viz".into(),
            project_name: "data-viz".into(),
            started_at: now - 5 * 60 * 1000, // 5m ago
            status: SessionStatus::Thinking,
            model: "gpt-5.4".into(),
            effort: "medium".into(),
            context_percent: 18.0,
            total_input_tokens: 3_100,
            total_output_tokens: 1_400,
            total_cache_read: 85_000,
            total_cache_create: 12_000,
            model_usage: Vec::new(),
            turn_count: 6,
            current_tasks: vec!["Write src/charts/heatmap.py".into()],
            mem_mb: 64,
            version: "0.116.0".into(),
            git_branch: "feat/heatmap".into(),
            git_added: 3,
            git_modified: 1,
            token_history: vec![5000, 8000, 12000, 18000, 15000, 22000],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagent_counts: Default::default(),
            subagents: vec![],
            pending_workflows: 0,
            pending_bg_agents: 0,
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![ChildProcess {
                pid: 8950,
                command: "python -m http.server 8080".into(),
                mem_kb: 32_000,
                port: Some(8080),
            }],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Create interactive heatmap component with D3.js".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Building the component now; next check is responsive canvas sizing.".into(),
                },
            ],
            initial_prompt: "Create interactive heatmap component with D3.js".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.codex".into(),
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "opencode",
            launch_surface: LaunchSurface::Cli,
            pid: 9500,
            session_id: "ses_e5f6a7b8-9abc-def0-1234-555555555555".into(),
            cwd: "/Users/demo/infra".into(),
            project_name: "infra".into(),
            started_at: now - 8 * 60 * 1000, // 8m ago
            status: SessionStatus::Thinking,
            model: "ollama/qwen3:14b".into(),
            effort: String::new(),
            context_percent: 35.0,
            total_input_tokens: 14_200,
            total_output_tokens: 3_800,
            total_cache_read: 0,
            total_cache_create: 0,
            model_usage: Vec::new(),
            turn_count: 9,
            current_tasks: vec!["thinking...".into()],
            mem_mb: 156,
            version: "1.4.0".into(),
            git_branch: "main".into(),
            git_added: 1,
            git_modified: 3,
            token_history: vec![
                3000, 5000, 8000, 6000, 9000, 7000, 11000, 8000, 10000,
            ],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagent_counts: Default::default(),
            subagents: vec![],
            pending_workflows: 0,
            pending_bg_agents: 0,
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![],

            first_assistant_text: String::new(),
            chat_messages: vec![],
            initial_prompt: "Refactor Terraform modules for multi-region".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.local/share/opencode".into(),
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "claude",
            launch_surface: LaunchSurface::Cli,
            pid: 9731,
            session_id: "f6a7b8c9-abcd-ef01-2345-666666666666".into(),
            cwd: "/Users/demo/docs-site".into(),
            project_name: "docs-site".into(),
            started_at: now - 22 * 60 * 1000, // 22m ago
            status: SessionStatus::Background,
            model: "claude-opus-4-6".into(),
            effort: String::new(),
            context_percent: 28.0,
            total_input_tokens: 9_600,
            total_output_tokens: 4_100,
            total_cache_read: 240_000,
            total_cache_create: 38_000,
            model_usage: Vec::new(),
            turn_count: 7,
            current_tasks: vec!["workflow audit · Check links".into()],
            mem_mb: 118,
            version: "2.1.87".into(),
            git_branch: "docs/changelog".into(),
            git_added: 0,
            git_modified: 2,
            token_history: vec![4000, 9000, 6000, 12000, 8000, 5000, 7000],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagent_counts: Default::default(),
            subagents: vec![
                SubAgent {
                    activity_key: String::new(),
                    name: "Collect changelog entries".into(),
                    status: "done".into(),
                    tokens: 6_300,
                    workflow: Some("audit".into()),
                },
                SubAgent {
                    activity_key: String::new(),
                    name: "Check links".into(),
                    status: "working".into(),
                    tokens: 4_800,
                    workflow: Some("audit".into()),
                },
            ],
            pending_workflows: 1,
            pending_bg_agents: 0,
            mem_file_count: 1,
            mem_line_count: 4,
            children: vec![],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Audit the changelog and docs links before the release".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Started the audit workflow; it reports back when the link check finishes.".into(),
                },
            ],
            initial_prompt: "Audit the changelog and docs links before the release".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.claude".into(),
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "omp",
            launch_surface: LaunchSurface::Cli,
            pid: 9612,
            session_id: "01a2b3c4-d5e6-7f80-9a1b-777777777777".into(),
            cwd: "/Users/demo/mobile-app".into(),
            project_name: "mobile-app".into(),
            started_at: now - 14 * 60 * 1000, // 14m ago
            status: SessionStatus::Executing,
            model: "claude-sonnet-4-6".into(),
            effort: "high".into(),
            context_percent: 41.0,
            total_input_tokens: 7_800,
            total_output_tokens: 2_900,
            total_cache_read: 156_000,
            total_cache_create: 21_000,
            model_usage: Vec::new(),
            turn_count: 11,
            current_tasks: vec!["Edit src/screens/Checkout.tsx".into()],
            mem_mb: 212,
            version: "14.2.0".into(),
            git_branch: "feat/apple-pay".into(),
            git_added: 2,
            git_modified: 4,
            token_history: vec![6000, 9000, 14000, 11000, 16000, 12000, 18000],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagent_counts: Default::default(),
            subagents: vec![],
            pending_workflows: 0,
            pending_bg_agents: 0,
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Add Apple Pay to the checkout screen".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Wiring the payment sheet into Checkout; the button state comes next.".into(),
                },
            ],
            initial_prompt: "Add Apple Pay to the checkout screen".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.omp/agent".into(),
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "pi",
            launch_surface: LaunchSurface::Cli,
            pid: 9688,
            session_id: "01a2b3c4-d5e6-7f80-9a1b-888888888888".into(),
            cwd: "/Users/demo/cli-tools".into(),
            project_name: "cli-tools".into(),
            started_at: now - 3 * 60 * 1000, // 3m ago
            status: SessionStatus::Thinking,
            model: "gpt-5.4".into(),
            effort: "low".into(),
            context_percent: 9.0,
            total_input_tokens: 2_400,
            total_output_tokens: 700,
            total_cache_read: 31_000,
            total_cache_create: 4_000,
            model_usage: Vec::new(),
            turn_count: 3,
            current_tasks: vec!["thinking...".into()],
            mem_mb: 88,
            version: "1.0.0".into(),
            git_branch: "main".into(),
            git_added: 0,
            git_modified: 1,
            token_history: vec![2000, 4000, 3000],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagent_counts: Default::default(),
            subagents: vec![],
            pending_workflows: 0,
            pending_bg_agents: 0,
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Add a --json flag to the export command".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Adding --json to export first, then a snapshot test for the output.".into(),
                },
            ],
            initial_prompt: "Add a --json flag to the export command".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.pi/agent".into(),
            file_accesses: vec![],
        },
    ];

    for session in &mut app.sessions {
        let total = session.total_tokens();
        let child_tokens: u64 = session.subagents.iter().map(|child| child.tokens).sum();
        ModelUsage::add(
            &mut session.model_usage,
            &session.model,
            total - child_tokens,
        );
        ModelUsage::add(&mut session.model_usage, SUBAGENT_MODEL, child_tokens);
        // Synthetic values are shape weights; allocate only the parent's usage.
        let weight: u64 = session.token_history.iter().sum();
        let mut cumulative = 0;
        let mut previous = 0;
        for value in &mut session.token_history {
            cumulative += *value;
            let allocated = ((total - child_tokens) * cumulative)
                .checked_div(weight)
                .unwrap_or(0);
            *value = allocated - previous;
            previous = allocated;
        }
    }

    // --- Summaries (pre-populated, no LLM calls) ---
    app.summaries.insert(
        "a1b2c3d4-5678-9abc-def0-111111111111".into(),
        "Stripe payment integration".into(),
    );
    app.summaries.insert(
        "b2c3d4e5-6789-abcd-ef01-222222222222".into(),
        "Batch inference endpoint".into(),
    );
    app.summaries.insert(
        "c3d4e5f6-789a-bcde-f012-333333333333".into(),
        "CORS fix + rate limiting".into(),
    );
    app.summaries.insert(
        "d4e5f6a7-89ab-cdef-0123-444444444444".into(),
        "D3 heatmap component".into(),
    );
    app.summaries.insert(
        "ses_e5f6a7b8-9abc-def0-1234-555555555555".into(),
        "Terraform multi-region refactor".into(),
    );
    app.summaries.insert(
        "01a2b3c4-d5e6-7f80-9a1b-777777777777".into(),
        "Apple Pay checkout".into(),
    );
    app.summaries.insert(
        "01a2b3c4-d5e6-7f80-9a1b-888888888888".into(),
        "Export --json flag".into(),
    );
    app.summaries.insert(
        "f6a7b8c9-abcd-ef01-2345-666666666666".into(),
        "Changelog and link audit".into(),
    );

    // --- Rate limits ---
    app.rate_limits = vec![
        RateLimitInfo {
            source: "claude".into(),
            five_hour_pct: Some(35.0),
            five_hour_resets_at: Some(now_secs() + 3 * 3600),
            five_hour_window_minutes: Some(300),
            seven_day_pct: Some(12.0),
            seven_day_resets_at: Some(now_secs() + 5 * 24 * 3600),
            seven_day_window_minutes: Some(10_080),
            updated_at: Some(now_secs() - 10),
        },
        RateLimitInfo {
            source: "codex".into(),
            five_hour_pct: Some(9.0),
            five_hour_resets_at: Some(now_secs() + 4 * 3600),
            five_hour_window_minutes: Some(300),
            seven_day_pct: Some(14.0),
            seven_day_resets_at: Some(now_secs() + 6 * 24 * 3600),
            seven_day_window_minutes: Some(10_080),
            updated_at: Some(now_secs() - 5),
        },
    ];

    // --- Token activity (synthetic, replayed through the real rate accounting) ---
    app.clear_global_token_history();
    app.live_token_history.clear();
    let now = Instant::now();
    // Replay enough steps to fill the Agents pane's whole window.
    let steps = (AGENT_HISTORY_WINDOW.as_secs() / DEMO_STEP.as_secs()) as usize;
    let first = demo_step().wrapping_sub(steps - 1);
    // Baseline the per-session counters before the replayed history starts.
    app.update_session_token_rates(now - DEMO_STEP * steps as u32);
    for offset in 0..steps {
        let at = now - DEMO_STEP * (steps - 1 - offset) as u32;
        apply_demo_step(app, first.wrapping_add(offset), at);
    }

    // --- Orphan ports ---
    app.orphan_ports = vec![OrphanPort {
        port: 4000,
        pid: 6543,
        command: "node dist/server.js".into(),
        project_name: "old-project".into(),
    }];

    // --- Host metrics + agent aggregate (demo values) ---
    app.host_metrics = Some(crate::host_info::HostMetrics {
        cpu_pct: 23.0,
        mem_pct: 41.0,
        load1: 1.8,
    });
    app.agent_aggregate = crate::host_info::AgentAggregate::from_sessions(&app.sessions);
}

/// Synthetic tokens per refresh; the chart loops over this shape.
const DEMO_RATES: [u64; 50] = [
    0, 0, 120, 340, 580, 420, 0, 0, 890, 1200, 950, 680, 0, 0, 450, 780, 1100, 1350, 920, 610, 0,
    340, 670, 890, 1050, 780, 520, 0, 0, 1400, 1180, 850, 620, 410, 0, 560, 820, 1060, 1280, 940,
    700, 480, 0, 0, 380, 720, 1150, 1320, 980, 650,
];
const DEMO_STEP: Duration = Duration::from_secs(2);
const SUBAGENT_MODEL: &str = "claude-haiku-4-5";

fn demo_step() -> usize {
    (now_secs() / DEMO_STEP.as_secs()) as usize
}

/// Grows the counters of working sessions by one step of the synthetic shape,
/// then lets the app derive global, per-session, and per-model rates from them.
fn apply_demo_step(app: &mut App, step: usize, at: Instant) {
    let rate = DEMO_RATES[step % DEMO_RATES.len()];
    let cache_read = rate * (step as u64 % 5 + 2);
    let weights: Vec<u64> = app
        .sessions
        .iter()
        .enumerate()
        .map(|(i, session)| match session.status.is_active() {
            true => ((step + i) % 4 + 1) as u64,
            false => 0,
        })
        .collect();
    let weight_sum: u64 = weights.iter().sum();
    for (i, session) in app.sessions.iter_mut().enumerate() {
        if weight_sum > 0 {
            let share = |tokens: u64| tokens * weights[i] / weight_sum;
            let (input, output) = (share(rate / 2), share(rate / 4));
            let cache_write = share(rate - rate / 2 - rate / 4);
            session.total_input_tokens += input;
            session.total_output_tokens += output;
            session.total_cache_create += cache_write;
            if session.total_cache_read > 0 {
                session.total_cache_read += share(cache_read);
            }
            add_demo_usage(session, input + output + cache_write);
        }
        if matches!(session.agent_cli, "claude" | "codex" | "omp" | "pi") {
            // Busier steps run more subagents, so both charts share a shape.
            session.subagent_counts.working = match rate {
                0 => 0,
                _ => (rate / 400 + (step + i) as u64 % 2) as u32,
            };
        }
    }
    app.update_session_token_rates(at);
    app.record_agents(at);
}

/// A Background root only waits, so its working subagents take all of its
/// usage; other roots with a working subagent hand it a third.
fn add_demo_usage(session: &mut AgentSession, tokens: u64) {
    let model = session.model.clone();
    let Some(child) = session.subagents.iter_mut().find(|c| c.status == "working") else {
        ModelUsage::add(&mut session.model_usage, &model, tokens);
        return;
    };
    let child_tokens = match session.status {
        SessionStatus::Background => tokens,
        _ => tokens / 3,
    };
    child.tokens += child_tokens;
    ModelUsage::add(&mut session.model_usage, SUBAGENT_MODEL, child_tokens);
    ModelUsage::add(&mut session.model_usage, &model, tokens - child_tokens);
}

pub(crate) fn advance_token_rates(app: &mut App) {
    apply_demo_step(app, demo_step(), Instant::now());
}
