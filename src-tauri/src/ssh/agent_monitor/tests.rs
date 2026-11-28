use super::parsers::*;
use super::quota::*;
use super::statusline::*;
use super::types::*;
use super::*;
use crate::ssh::system_monitor::{local_os, native_remote_path, remote_join};
#[cfg(any(target_os = "macos", windows))]
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
#[cfg(any(target_os = "macos", windows))]
use std::io::Write;
use std::path::Path;
#[cfg(any(target_os = "macos", windows))]
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
#[cfg(any(target_os = "macos", windows))]
use std::time::{SystemTime, UNIX_EPOCH};


/// Fixed clock for the parsers, well ahead of the fixture reset timestamps.
const TEST_NOW: u64 = 1_799_000_000;

fn parse_metadata(provider: Provider, lines: &[String]) -> AgentSessionMetadata {
    parse_provider_metadata(provider, lines, TEST_NOW)
}

#[test]
fn detects_agents_and_aggregates_child_resources() {
    let processes = parse_posix_processes(
        "100 1 alice 2.0 100000 01:00 /usr/bin/codex\n\
         101 100 alice 4.0 50000 00:30 node worker.js\n\
         200 1 bob 1.0 70000 00:20 node /x/@anthropic-ai/claude-code/cli.js\n",
    );
    let metrics = build_agent_metrics(&processes, &HashMap::new(), &HashMap::new());
    assert_eq!(metrics.len(), 2);
    let codex = metrics
        .iter()
        .find(|metric| metric.provider == "codex")
        .unwrap();
    assert_eq!(codex.process_count, 2);
    assert_eq!(codex.cpu_percent, Some(6.0));
    assert_eq!(codex.memory_bytes, Some(150_000 * 1024));
}

#[test]
fn parses_codex_token_and_rate_limit_snapshot() {
    let lines = vec![
        r#"{"type":"session_meta","payload":{"id":"session-1","cwd":"/work","model_provider":"openai"}}"#.to_string(),
        r#"{"type":"turn_context","payload":{"model":"gpt-5.4"}}"#.to_string(),
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1200,"output_tokens":300,"total_tokens":1500},"last_token_usage":{"total_tokens":500},"model_context_window":10000},"rate_limits":{"primary":{"used_percent":42,"window_minutes":10080,"resets_at":1800500000}}}}"#.to_string(),
    ];
    let metadata = parse_metadata(Provider::Codex, &lines);
    assert_eq!(metadata.session_id.as_deref(), Some("session-1"));
    assert_eq!(metadata.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(metadata.total_tokens, Some(1500));
    assert_eq!(metadata.context_used_percent, Some(5.0));
    assert_eq!(metadata.rate_limits[0].used_percent, Some(42.0));
    assert_eq!(metadata.rate_limits[0].window_minutes, Some(10080));
    assert_eq!(metadata.rate_limits[0].group, None);
}

#[test]
fn parses_agy_status_payload_without_prompt_content() {
    let lines = vec![r#"{"conversation_id":"agy-1","model":{"display_name":"Gemini"},"agent_state":"working","input_tokens":80,"output_tokens":20,"total_tokens":100,"context_window":{"context_used_tokens":100,"context_window_size":1000},"quota":{"premium":{"remaining_fraction":0.7}},"subagents":[{"name":"tests","role":"worker","status":"active"}],"background_tasks":[{"name":"npm test","status":"running"}],"prompt":"do not expose this prompt"}"#.to_string()];
    let metadata = parse_metadata(Provider::Agy, &lines);
    assert_eq!(metadata.status.as_deref(), Some("working"));
    assert_eq!(metadata.model.as_deref(), Some("Gemini"));
    assert_eq!(metadata.total_tokens, Some(100));
    assert_eq!(metadata.context_used_percent, Some(10.0));
    assert_eq!(metadata.context_remaining_tokens, Some(900));
    assert_eq!(metadata.context_remaining_percent, Some(90.0));
    assert_eq!(metadata.rate_limits[0].used_percent, Some(30.0));
    assert_eq!(metadata.subagents.len(), 1);
    assert_eq!(metadata.background_tasks.len(), 1);

    let processes =
        parse_posix_processes("100 1 alice 2.0 100000 01:00 agy --api-key do-not-expose\n");
    let mut sessions = HashMap::new();
    sessions.insert(Provider::Agy, vec![metadata]);
    let serialized =
        serde_json::to_string(&build_agent_metrics(&processes, &sessions, &HashMap::new()))
            .unwrap();
    assert!(!serialized.contains("do not expose this prompt"));
    assert!(!serialized.contains("do-not-expose"));
}

#[test]
fn parses_agy_grouped_weekly_and_five_hour_limits() {
    let lines = vec![r#"{
        "quota": {
            "gemini_models": {
                "display_name": "Gemini models",
                "weekly_limit": {"remaining_percentage": 99.9, "refreshes_at": 1800000000},
                "five_hour_limit": {"remaining_percentage": 99.4, "refreshes_at": 1799990000}
            },
            "claude_and_gpt_models": {
                "display_name": "Claude and GPT models",
                "weekly_limit": {"remaining_percentage": 100},
                "five_hour_limit": {"remaining_percentage": 100}
            }
        }
    }"#
    .to_string()];
    let metadata = parse_metadata(Provider::Agy, &lines);
    assert_eq!(metadata.rate_limits.len(), 4);
    let gemini_weekly = metadata
        .rate_limits
        .iter()
        .find(|limit| {
            limit.group.as_deref() == Some("Gemini models")
                && limit.window_minutes == Some(10080)
        })
        .unwrap();
    assert_eq!(gemini_weekly.group.as_deref(), Some("Gemini models"));
    assert_eq!(gemini_weekly.window_minutes, Some(10080));
    assert!((gemini_weekly.used_percent.unwrap() - 0.1).abs() < 1e-9);
    assert!(metadata.rate_limits.iter().any(|limit| {
        limit.group.as_deref() == Some("Gemini models") && limit.window_minutes == Some(300)
    }));
    assert!(metadata.rate_limits.iter().any(|limit| {
        limit.group.as_deref() == Some("Claude and GPT models")
            && limit.window_minutes == Some(10080)
    }));
}

#[test]
fn parses_claude_context_and_rate_limits_without_faking_session_totals() {
    let lines = vec![
        r#"{"sessionId":"claude-1","message":{"id":"msg-1","model":"claude-sonnet","usage":{"input_tokens":100,"cache_read_input_tokens":50,"output_tokens":20}}}"#.to_string(),
        r#"{"sessionId":"claude-1","message":{"id":"msg-1","model":"claude-sonnet","usage":{"input_tokens":100,"cache_read_input_tokens":50,"output_tokens":20}}}"#.to_string(),
        r#"{"total_cost_usd":0.42,"total_duration_ms":5000,"context_window":{"total_input_tokens":150,"total_output_tokens":20,"context_window_size":1000,"used_percentage":15,"remaining_percentage":85},"rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1800300000},"seven_day":{"used_percentage":41.2,"resets_at":1800900000}}}"#.to_string(),
    ];
    let metadata = parse_metadata(Provider::Claude, &lines);
    // `context_window.*` is the live context, so it must not be reported as
    // a cumulative session total.
    assert_eq!(metadata.input_tokens, None);
    assert_eq!(metadata.output_tokens, None);
    assert_eq!(metadata.total_tokens, None);
    assert_eq!(metadata.context_used_tokens, Some(150));
    assert_eq!(metadata.last_request_input_tokens, Some(100));
    assert_eq!(metadata.last_request_cache_read_tokens, Some(50));
    assert_eq!(metadata.last_request_output_tokens, Some(20));
    assert_eq!(metadata.cost_usd, Some(0.42));
    assert_eq!(metadata.session_duration_seconds, Some(5.0));
    assert_eq!(metadata.context_remaining_tokens, Some(850));
    assert_eq!(metadata.context_remaining_percent, Some(85.0));
    assert_eq!(metadata.rate_limits.len(), 2);
    assert_eq!(metadata.rate_limits[0].window_minutes, Some(300));
    assert_eq!(metadata.rate_limits[1].window_minutes, Some(10080));
}

#[test]
fn merges_status_snapshot_into_matching_agent_session() {
    let output = concat!(
        "__GPUTERM_AGENT_FILE__\tagy\tconversation.db\n",
        "{\"conversation_id\":\"same\",\"input_tokens\":100,\"output_tokens\":20,\"context_window\":{\"context_used_tokens\":80,\"context_window_size\":1000}}\n",
        "__GPUTERM_AGENT_END__\n",
        "__GPUTERM_AGENT_FILE__\tagy\tstatus.json\n",
        "{\"conversation_id\":\"same\",\"agent_state\":\"working\",\"quota\":{\"premium\":{\"remaining_percentage\":75}}}\n",
        "__GPUTERM_AGENT_END__\n",
    );
    let grouped = parse_metadata_output(output, TEST_NOW);
    let sessions = grouped.get(&Provider::Agy).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].input_tokens, Some(100));
    assert_eq!(sessions[0].status.as_deref(), Some("working"));
    assert_eq!(sessions[0].rate_limits[0].used_percent, Some(25.0));
}

#[test]
fn detects_agents_launched_from_long_absolute_paths() {
    // macOS truncates the `comm` column, so detection has to work from the
    // command line alone.
    let processes = parse_posix_processes(concat!(
        "100 1 alice 1.0 1000 01:00 /Users/a/Library/Application Support/Claude/claude-code/2.1.219/claude.app/Contents/MacOS/claude --output-format stream-json\n",
        "101 1 alice 1.0 1000 01:00 /Applications/ChatGPT.app/Contents/Resources/codex -c features.code_mode_host=true app-server\n",
        "102 1 alice 1.0 1000 01:00 codex exec --sandbox read-only\n",
        "103 1 alice 1.0 1000 01:00 /Applications/Claude.app/Contents/MacOS/Claude\n",
        "104 1 alice 1.0 1000 01:00 /Applications/ChatGPT.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer) --type=renderer\n",
    ));
    let detected = processes
        .iter()
        .filter_map(|process| {
            provider_for_process(process).map(|provider| (process.pid, provider))
        })
        .collect::<HashMap<_, _>>();
    assert_eq!(detected.get(&100), Some(&Provider::Claude));
    assert_eq!(detected.get(&101), Some(&Provider::Codex));
    assert_eq!(detected.get(&102), Some(&Provider::Codex));
    // The desktop shell and its renderer helpers are not agent sessions.
    assert_eq!(detected.get(&103), None);
    assert_eq!(detected.get(&104), None);
}

#[test]
fn ignores_subagent_records_so_they_cannot_overwrite_the_session() {
    // The collector can surface a session transcript and a worker
    // transcript that share a session id; only the session's own records
    // may describe its context.
    let output = concat!(
        "__GPUTERM_AGENT_FILE__\tclaude\tsession.jsonl\n",
        "{\"sessionId\":\"claude-1\",\"cwd\":\"/work\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":90000,\"cache_read_input_tokens\":10000,\"output_tokens\":500}}}\n",
        "__GPUTERM_AGENT_END__\n",
        "__GPUTERM_AGENT_FILE__\tclaude\tsubagents/agent-1.jsonl\n",
        "{\"sessionId\":\"claude-1\",\"isSidechain\":true,\"agentId\":\"agent-1\",\"message\":{\"id\":\"msg-2\",\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":0,\"output_tokens\":5}}}\n",
        "__GPUTERM_AGENT_END__\n",
    );
    let grouped = parse_metadata_output(output, TEST_NOW);
    let sessions = grouped.get(&Provider::Claude).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].context_used_tokens, Some(100_000));
    assert_eq!(sessions[0].last_request_output_tokens, Some(500));
}

#[test]
fn attributes_snapshots_to_their_own_process() {
    // Pid 201 is a launcher wrapping the agent at 200, as the Claude desktop
    // app does, so the snapshot names a pid below the tree root.
    let processes = parse_posix_processes(
        "201 1 alice 0.1 1000 00:21 launcher /x/@anthropic-ai/claude-code/cli.js\n\
         200 201 alice 1.0 70000 00:20 node /x/@anthropic-ai/claude-code/cli.js\n\
         100 1 alice 1.0 70000 00:40 node /x/@anthropic-ai/claude-code/cli.js\n",
    );
    let mut sessions = HashMap::new();
    sessions.insert(
        Provider::Claude,
        vec![
            AgentSessionMetadata {
                session_id: Some("newest".to_string()),
                pid_hint: Some(200),
                context_used_tokens: Some(200),
                ..Default::default()
            },
            AgentSessionMetadata {
                session_id: Some("older".to_string()),
                pid_hint: Some(100),
                context_used_tokens: Some(100),
                ..Default::default()
            },
        ],
    );
    let metrics = build_agent_metrics(&processes, &sessions, &HashMap::new());
    let by_pid = metrics
        .iter()
        .map(|metric| (metric.root_pid, metric))
        .collect::<HashMap<_, _>>();
    assert_eq!(by_pid[&100].session_id.as_deref(), Some("older"));
    assert_eq!(by_pid[&100].context_used_tokens, Some(100));
    assert_eq!(by_pid[&201].session_id.as_deref(), Some("newest"));
    assert_eq!(by_pid[&201].context_used_tokens, Some(200));
    assert!(
        !by_pid.contains_key(&200),
        "the launcher tree owns the agent"
    );
}

#[test]
fn keeps_codex_quota_when_a_later_event_omits_it() {
    let lines = vec![
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1000},"model_context_window":10000},"rate_limits":{"limit_id":"codex","plan_type":"plus","credits":{"has_credits":false,"balance":"0"},"primary":{"used_percent":2,"window_minutes":300,"resets_at":1799500000},"secondary":{"used_percent":17,"window_minutes":10080,"resets_at":1799900000}}}}"#.to_string(),
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":2000},"model_context_window":10000}}}"#.to_string(),
    ];
    let metadata = parse_metadata(Provider::Codex, &lines);
    assert_eq!(metadata.total_tokens, Some(2000));
    assert_eq!(metadata.rate_limits.len(), 2);
    assert!(metadata
        .rate_limits
        .iter()
        .all(|limit| !limit.stale && limit.used_percent.is_some()));
}

#[test]
fn marks_rolled_over_windows_as_stale_and_accepts_relative_resets() {
    let lines = vec![r#"{"rate_limits":{"five_hour":{"used_percentage":80,"resets_at":1798000000},"seven_day":{"used_percentage":40,"resets_in_seconds":3600}}}"#.to_string()];
    let metadata = parse_metadata(Provider::Claude, &lines);
    let five_hour = metadata
        .rate_limits
        .iter()
        .find(|limit| limit.window_minutes == Some(300))
        .unwrap();
    assert!(five_hour.stale);
    let weekly = metadata
        .rate_limits
        .iter()
        .find(|limit| limit.window_minutes == Some(10080))
        .unwrap();
    assert!(!weekly.stale);
    assert_eq!(weekly.resets_at, Some(TEST_NOW + 3600));
}

#[test]
fn normalizes_millisecond_reset_timestamps_to_seconds() {
    // Both reset renderers in the webview and the staleness check here read
    // seconds, so a millisecond-emitting provider must be converted once at
    // the parse boundary rather than guessed at each display site.
    let lines = vec![
        r#"{"rate_limits":{"five_hour":{"used_percentage":10,"resets_at":1799500000000}}}"#
            .to_string(),
    ];
    let metadata = parse_metadata(Provider::Claude, &lines);
    let limit = &metadata.rate_limits[0];
    assert_eq!(limit.resets_at, Some(1_799_500_000));
    assert!(!limit.stale);
}

#[test]
fn reports_snapshot_age_from_capture_time() {
    let lines = vec![r#"{"session_id":"claude-1","captured_at":1798999880,"pid":4242,"rate_limits":{"five_hour":{"used_percentage":10,"resets_at":1799500000}}}"#.to_string()];
    let metadata = parse_metadata(Provider::Claude, &lines);
    assert_eq!(metadata.snapshot_age_seconds, Some(120));
    assert_eq!(metadata.pid_hint, Some(4242));
}

#[test]
fn builds_metadata_command_only_for_running_providers() {
    let claude_only = metadata_command(RemoteOs::Linux, &HashSet::from([Provider::Claude]));
    assert!(claude_only.contains("emit_agent_files claude"));
    assert!(claude_only.contains("emit_agent_snapshots claude"));
    assert!(!claude_only.contains("emit_agent_files codex"));
    assert!(!claude_only.contains("sqlite3"));

    let agy_only = metadata_command(RemoteOs::Linux, &HashSet::from([Provider::Agy]));
    assert!(agy_only.contains("sqlite3"));
    assert!(agy_only.contains("emit_agent_snapshots agy"));
    assert!(!agy_only.contains("emit_agent_files claude"));

    let windows = metadata_command(RemoteOs::Windows, &HashSet::from([Provider::Claude]));
    assert!(windows.contains("Emit-AgentFiles 'claude'"));
    assert!(windows.contains("Emit-AgentSnapshots 'claude'"));
    assert!(windows.trim_end().ends_with("exit 0"));
}

#[test]
fn codex_live_account_response_normalizes_used_to_remaining() {
    let response = serde_json::json!({
        "id": 2,
        "result": {
            "rateLimits": {
                "limitId": "codex",
                "primary": {
                    "usedPercent": 3,
                    "windowDurationMins": 10080,
                    "resetsAt": TEST_NOW + 3600
                },
                "secondary": null
            },
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "primary": {
                        "usedPercent": 3,
                        "windowDurationMins": 10080,
                        "resetsAt": TEST_NOW + 3600
                    },
                    "secondary": null
                }
            }
        }
    });
    let quota = parse_codex_quota_response(&response, TEST_NOW).unwrap();
    assert_eq!(quota.source, "codex-app-server");
    assert_eq!(quota.limits.len(), 1);
    assert_eq!(quota.limits[0].window_minutes, Some(10080));
    assert_eq!(quota.limits[0].used_percent, Some(3.0));
    assert_eq!(quota.limits[0].remaining_percent, Some(97.0));
}

#[test]
fn reports_the_blocking_setup_step_instead_of_a_generic_absence() {
    let cases = [
        ("missing", "ours", "not installed"),
        ("empty", "ours", "present but empty"),
        ("ok", "none", "no status line configured"),
        ("ok", "other", "different status line"),
        ("ok", "ours", "Send one message"),
    ];
    for (helper, status_line, expected) in cases {
        let output = format!(
            "__GPUTERM_AGENT_FILE__\tclaude\tsetup-state\n\
             {{\"scope\":\"setup\",\"helper\":\"{helper}\",\"status_line\":\"{status_line}\"}}\n\
             __GPUTERM_AGENT_END__\n"
        );
        let mut state = AgentMonitorState {
            metadata: parse_metadata_output(&output, TEST_NOW),
            ..Default::default()
        };
        merge_metadata_quota_fallbacks(
            &mut state,
            &HashSet::from([Provider::Claude]),
            TEST_NOW,
        );
        let quota = state.quotas.get(&Provider::Claude).unwrap();
        assert_eq!(quota.status, "setup-required");
        let message = quota.message.as_deref().unwrap_or_default();
        assert!(
            message.contains(expected),
            "helper={helper} status_line={status_line} produced {message:?}"
        );
    }
}

#[test]
fn a_failed_metadata_scrape_reaches_the_card_instead_of_vanishing() {
    // The Windows symptom was a card with no model and a generic quota
    // message, because the scrape error was discarded. Any remaining cause
    // has to be visible.
    let mut state = AgentMonitorState::default();
    refresh_metadata_if_due(
        &mut state,
        RemoteOs::Windows,
        &HashSet::from([Provider::Claude]),
        |_| Err("The command line is too long.".to_string()),
    );
    let quota = state.quotas.get(&Provider::Claude).unwrap();
    assert_eq!(quota.status, "setup-required");
    let message = quota.message.as_deref().unwrap_or_default();
    assert!(message.contains("could not be collected"), "{message}");
    assert!(message.contains("command line is too long"), "{message}");
}

#[test]
fn a_specific_setup_step_outranks_the_collection_error() {
    // When the collector did report the setup state, that is the more
    // actionable message.
    let setup = AgentSessionMetadata {
        setup_scope: true,
        setup_helper: Some("empty".to_string()),
        setup_status_line: Some("ours".to_string()),
        ..Default::default()
    };
    let snapshot = unavailable_with_setup_hint(
        Provider::Claude,
        Some(&setup),
        Some("some transport error"),
    );
    let message = snapshot.message.as_deref().unwrap_or_default();
    assert!(message.contains("present but empty"), "{message}");
    assert!(!message.contains("some transport error"), "{message}");
}

#[test]
fn setup_state_record_is_not_treated_as_a_session() {
    let output = concat!(
        "__GPUTERM_AGENT_FILE__\tclaude\tsetup-state\n",
        "{\"scope\":\"setup\",\"helper\":\"ok\",\"status_line\":\"ours\"}\n",
        "__GPUTERM_AGENT_END__\n",
    );
    let grouped = parse_metadata_output(output, TEST_NOW);
    let sessions = grouped.get(&Provider::Claude).unwrap();
    assert!(sessions.iter().all(|entry| entry.setup_scope));
    // It names no session, so it must never become an agent card's metadata.
    assert!(sessions.iter().all(|entry| entry.session_id.is_none()));
}

#[test]
fn remote_paths_use_sftp_separators_and_native_windows_drives() {
    assert_eq!(
        remote_join(Path::new("/home/alice"), ".claude"),
        "/home/alice/.claude"
    );
    assert_eq!(remote_join(Path::new("/home/alice/"), ".claude"), "/home/alice/.claude");
    // Windows OpenSSH reports the home directory in this form.
    assert_eq!(
        remote_join(Path::new("/C:/Users/Test User"), ".claude"),
        "/C:/Users/Test User/.claude"
    );
    // A `-File` argument needs the drive without the SFTP root slash.
    assert_eq!(
        native_remote_path("/C:/Users/Test User/.claude/x.ps1"),
        "C:/Users/Test User/.claude/x.ps1"
    );
    assert_eq!(native_remote_path("/home/alice/.claude/x.sh"), "/home/alice/.claude/x.sh");
}

#[test]
fn claude_settings_parsing_tolerates_a_bom_and_an_empty_file() {
    // PowerShell redirection and Windows editors both write a UTF-8 BOM,
    // which `serde_json` rejects outright.
    let with_bom = b"\xef\xbb\xbf{\"statusLine\":{\"command\":\"x\"}}";
    let parsed = parse_claude_settings(with_bom).expect("BOM is tolerated");
    assert_eq!(claude_status_line_command(&parsed).as_deref(), Some("x"));

    assert!(parse_claude_settings(b"").unwrap().is_object());
    assert!(parse_claude_settings(b"   \n").unwrap().is_object());
    assert!(parse_claude_settings(b"not json").is_err());
}

#[test]
fn account_snapshot_supplies_the_quota_when_every_session_lacks_limits() {
    // The situation observed on a real machine: Claude only publishes
    // rate limits after a session's first API response, so short-lived
    // sessions kept writing newer, quota-less snapshots and pushed the only
    // useful reading out of the collector's newest-files window.
    let mut output = String::from(concat!(
        "__GPUTERM_AGENT_FILE__\tclaude\taccount.json\n",
        "{\"scope\":\"account\",\"captured_at\":1798999900,\"session_id\":\"published\",\"rate_limits\":{\"five_hour\":{\"used_percentage\":20,\"resets_at\":1799500000},\"seven_day\":{\"used_percentage\":40,\"resets_at\":1799900000}}}\n",
        "__GPUTERM_AGENT_END__\n",
    ));
    for index in 0..4 {
        output.push_str(&format!(
            "__GPUTERM_AGENT_FILE__\tclaude\tshort-{index}.json\n\
             {{\"session_id\":\"short-{index}\",\"captured_at\":1798999990,\"cwd\":\"/w\",\"cost\":{{\"total_cost_usd\":0.0,\"total_duration_ms\":236}}}}\n\
             __GPUTERM_AGENT_END__\n"
        ));
    }

    let mut state = AgentMonitorState {
        metadata: parse_metadata_output(&output, TEST_NOW),
        ..Default::default()
    };
    merge_metadata_quota_fallbacks(&mut state, &HashSet::from([Provider::Claude]), TEST_NOW);

    let quota = state.quotas.get(&Provider::Claude).unwrap();
    assert_eq!(quota.status, "available");
    assert_eq!(quota.source, "claude-statusline");
    let five_hour = quota
        .limits
        .iter()
        .find(|limit| limit.window_minutes == Some(300))
        .expect("five hour window");
    assert_eq!(five_hour.remaining_percent, Some(80.0));
}

#[test]
fn account_snapshot_does_not_overwrite_its_session_context() {
    // The account record names the session that published it, so merging by
    // session id would let it wipe that session's own cost and context.
    let output = concat!(
        "__GPUTERM_AGENT_FILE__\tclaude\tpublished.json\n",
        "{\"session_id\":\"published\",\"captured_at\":1798999900,\"cwd\":\"/work\",\"cost\":{\"total_cost_usd\":1.25,\"total_duration_ms\":165296},\"context_window\":{\"total_input_tokens\":150,\"context_window_size\":1000}}\n",
        "__GPUTERM_AGENT_END__\n",
        "__GPUTERM_AGENT_FILE__\tclaude\taccount.json\n",
        "{\"scope\":\"account\",\"captured_at\":1798999950,\"session_id\":\"published\",\"rate_limits\":{\"five_hour\":{\"used_percentage\":20,\"resets_at\":1799500000}}}\n",
        "__GPUTERM_AGENT_END__\n",
    );
    let grouped = parse_metadata_output(output, TEST_NOW);
    let sessions = grouped.get(&Provider::Claude).unwrap();
    let session = sessions
        .iter()
        .find(|entry| !entry.account_scope)
        .expect("session record");
    assert_eq!(session.cwd.as_deref(), Some("/work"));
    assert_eq!(session.cost_usd, Some(1.25));
    assert_eq!(session.context_used_tokens, Some(150));
    assert!(sessions.iter().any(|entry| entry.account_scope));
}

#[test]
fn no_windows_command_is_ever_sent_inline_past_the_cmd_exe_limit() {
    use crate::ssh::system_monitor::{
        windows_encoded_command, windows_script_needs_upload, WINDOWS_CMD_EXE_LIMIT,
    };

    // This is the invariant that was silently violated: base64 of UTF-16LE
    // inflates a script about 2.7x, so the all-providers metadata scrape
    // reached roughly 18,000 characters against cmd.exe's 8,191 limit and
    // the whole scrape failed, leaving the card with no model and no quota.
    let mut scripts = vec![("process list".to_string(), WINDOWS_PROCESS_COMMAND.to_string())];
    let providers = [Provider::Claude, Provider::Codex, Provider::Agy];
    for mask in 1..(1 << providers.len()) {
        let combination = providers
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, provider)| *provider)
            .collect::<HashSet<_>>();
        let label = combination
            .iter()
            .map(|provider| provider.key())
            .collect::<Vec<_>>()
            .join("+");
        scripts.push((label, metadata_command(RemoteOs::Windows, &combination)));
    }

    for (label, script) in scripts {
        if windows_script_needs_upload(&script) {
            // Delivered by path, so its size no longer matters.
            continue;
        }
        let wire = windows_encoded_command(&script).len();
        assert!(
            wire <= WINDOWS_CMD_EXE_LIMIT,
            "{label} would be sent inline at {wire} characters, past cmd.exe's {WINDOWS_CMD_EXE_LIMIT}"
        );
    }
}

#[test]
fn the_all_providers_metadata_scrape_is_uploaded_rather_than_inlined() {
    use crate::ssh::system_monitor::windows_script_needs_upload;

    // The combination the reporting user actually runs on Windows.
    let script = metadata_command(
        RemoteOs::Windows,
        &HashSet::from([Provider::Claude, Provider::Codex, Provider::Agy]),
    );
    assert!(windows_script_needs_upload(&script));
    // Process detection stays inline: it is small and runs every poll.
    assert!(!windows_script_needs_upload(WINDOWS_PROCESS_COMMAND));
}

#[test]
fn collector_emits_the_account_snapshot_by_name() {
    let posix = metadata_command(RemoteOs::MacOs, &HashSet::from([Provider::Claude]));
    assert!(posix.contains("emit_agent_tail \"$provider\" \"$dir/account.json\""));
    // The recency-limited scan must skip it, or it would occupy one of the
    // four session slots.
    assert!(posix.contains("! -name 'account.json'"));

    let windows = metadata_command(RemoteOs::Windows, &HashSet::from([Provider::Claude]));
    assert!(windows.contains("Emit-AgentTail $provider (Join-Path $dir 'account.json')"));
    assert!(windows.contains("$_.Name -ne 'account.json'"));
}

#[test]
fn newest_account_quota_is_shared_instead_of_assigned_by_session() {
    let older = AgentSessionMetadata {
        session_id: Some("older".to_string()),
        captured_at: Some(TEST_NOW - 100),
        rate_limits: vec![AgentRateLimitMetric {
            label: "primary".to_string(),
            remaining_percent: Some(12.0),
            used_percent: Some(88.0),
            window_minutes: Some(10080),
            ..Default::default()
        }],
        ..Default::default()
    };
    let newest = AgentSessionMetadata {
        session_id: Some("newest".to_string()),
        captured_at: Some(TEST_NOW - 5),
        rate_limits: vec![AgentRateLimitMetric {
            label: "primary".to_string(),
            remaining_percent: Some(97.0),
            used_percent: Some(3.0),
            window_minutes: Some(10080),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut state = AgentMonitorState::default();
    state.metadata.insert(Provider::Codex, vec![older, newest]);
    merge_metadata_quota_fallbacks(&mut state, &HashSet::from([Provider::Codex]), TEST_NOW);
    let quota = state.quotas.get(&Provider::Codex).unwrap();
    assert_eq!(quota.source, "codex-session-log");
    assert_eq!(quota.limits[0].remaining_percent, Some(97.0));

    let processes = parse_posix_processes(
        "100 1 alice 1.0 1000 01:00 codex exec first\n\
         200 1 alice 1.0 1000 01:00 codex exec second\n",
    );
    let metrics = build_agent_metrics(&processes, &state.metadata, &state.quotas);
    assert_eq!(metrics.len(), 2);
    assert!(metrics
        .iter()
        .all(|metric| metric.quota.limits[0].remaining_percent == Some(97.0)));
}

#[test]
fn advances_quota_age_and_marks_windows_expired() {
    let mut state = AgentMonitorState::default();
    state.quotas.insert(
        Provider::Codex,
        AgentQuotaSnapshot::available(
            "codex-app-server",
            Some(TEST_NOW - 180),
            vec![AgentRateLimitMetric {
                label: "primary".to_string(),
                remaining_percent: Some(97.0),
                resets_at: Some(TEST_NOW + 30),
                ..Default::default()
            }],
            TEST_NOW - 180,
        ),
    );

    update_quota_snapshots(&mut state, TEST_NOW + 120);
    let quota = state.quotas.get(&Provider::Codex).unwrap();
    assert_eq!(quota.snapshot_age_seconds, Some(300));
    assert_eq!(quota.status, "stale");
    assert!(quota.limits[0].stale);
}

#[test]
fn parses_agy_usage_tui_groups_remaining_and_refresh_times() {
    let output = concat!(
        "\u{1b}[1mGEMINI MODELS\u{1b}[0m\r\n",
        "Models within this group: Gemini Flash, Gemini Pro\r\n",
        "Weekly Limit\r\n",
        "[||||] 99.90%\r\n",
        "100% remaining · Refreshes in 95h 58m\r\n",
        "Five Hour Limit\r\n",
        "[||||] 100.00%\r\n",
        "Quota available\r\n",
        "CLAUDE AND GPT MODELS\r\n",
        "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS\r\n",
        "Weekly Limit\r\n",
        "[||||] 100.00%\r\n",
        "Quota available\r\n",
        "Five Hour Limit\r\n",
        "[||||] 100.00%\r\n",
        "Quota available\r\n",
    );
    let quota = parse_agy_usage_output(output, TEST_NOW).unwrap();
    assert_eq!(quota.source, "agy-usage-tui");
    assert_eq!(quota.limits.len(), 4);
    assert_eq!(quota.limits[0].group.as_deref(), Some("Gemini models"));
    assert_eq!(quota.limits[0].remaining_percent, Some(99.9));
    assert_eq!(
        quota.limits[0].resets_at,
        Some(TEST_NOW + 95 * 3600 + 58 * 60)
    );
    assert_eq!(
        quota.limits[0].model_names,
        vec!["Gemini Flash", "Gemini Pro"]
    );
    assert_eq!(quota.limits[1].remaining_percent, Some(100.0));
    assert_eq!(
        quota.limits[2].group.as_deref(),
        Some("Claude and GPT models")
    );
    assert_eq!(quota.limits[2].remaining_percent, Some(100.0));
    assert_eq!(
        quota.limits[2].model_names,
        vec!["Claude Opus", "Claude Sonnet", "GPT-OSS"]
    );
}

#[test]
fn agy_usage_redraws_keep_the_latest_precise_value_and_wrapped_model_names() {
    let first = concat!(
        "GEMINI MODELS\n",
        "Models within this group: Gemini Flash, Gemini Pro\n",
        "Weekly Limit\n[||||] 80.25%\n80% remaining\n",
        "Five Hour Limit\n[||||] 90.50%\n91% remaining\n",
        "CLAUDE AND GPT MODELS\n",
        "Models within this group: Claude Opus, Claude\n",
        "Sonnet, GPT-OSS\n",
        "Weekly Limit\n[||||] 70.75%\n71% remaining\n",
        "Five Hour Limit\n[||||] 60.25%\n60% remaining\n",
    );
    let redraw = concat!(
        "GEMINI MODELS\n",
        "Models within this group: Gemini Flash, Gemini Pro\n",
        "Weekly Limit\n[||||] 79.95%\n80% remaining\n",
        "Five Hour Limit\n[||||] 89.75%\n90% remaining\n",
        "CLAUDE AND GPT MODELS\n",
        "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS\n",
        "Weekly Limit\n[||||] 69.50%\n70% remaining\n",
        "Five Hour Limit\n[||||] 59.95%\n60% remaining\n",
    );
    let quota = parse_agy_usage_output(&format!("{}{}", first, redraw), TEST_NOW).unwrap();
    assert_eq!(quota.limits.len(), 4);
    assert_eq!(quota.limits[0].remaining_percent, Some(79.95));
    assert_eq!(quota.limits[1].remaining_percent, Some(89.75));
    assert_eq!(quota.limits[2].remaining_percent, Some(69.5));
    assert_eq!(quota.limits[3].remaining_percent, Some(59.95));
    assert_eq!(
        quota.limits[2].model_names,
        vec!["Claude Opus", "Claude Sonnet", "GPT-OSS"]
    );
}

#[test]
fn parses_cursor_positioned_agy_output_and_joined_bar_status_lines() {
    let output = concat!(
        "\u{1b}[1;1HGEMINI MODELS",
        "\u{1b}[2;1HModels within this group: Gemini Flash, Gemini Pro",
        "\u{1b}[3;1HWeekly Limit",
        "\u{1b}[4;1H[||||] 99.90% 100% remaining · Refreshes in 95h 58m",
        "\u{1b}[5;1HFive Hour Limit",
        "\u{1b}[6;1H[||||] 100.00% Quota available",
        "\u{1b}[7;1HCLAUDE AND GPT MODELS",
        "\u{1b}[8;1HModels within this group: Claude Opus, Claude Sonnet, GPT-OSS",
        "\u{1b}[9;1HWeekly Limit",
        "\u{1b}[10;1H[||||] 100.00% Quota available",
        "\u{1b}[11;1HFive Hour Limit",
        "\u{1b}[12;1H[||||] 100.00% Quota available",
    );
    let quota = parse_agy_usage_output(output, TEST_NOW).unwrap();
    assert_eq!(quota.limits.len(), 4);
    assert_eq!(quota.limits[0].remaining_percent, Some(99.9));
    assert_eq!(
        quota.limits[0].resets_at,
        Some(TEST_NOW + 95 * 3600 + 58 * 60)
    );
    assert!(quota
        .limits
        .iter()
        .all(|limit| !limit.model_names.is_empty()));
}

#[test]
fn waits_for_both_agy_model_groups_before_finishing_the_probe() {
    let gemini_only = concat!(
        "GEMINI MODELS\n",
        "Weekly Limit\n100% remaining\n",
        "Five Hour Limit\n99% remaining\n",
    );
    assert!(!agy_usage_output_complete(gemini_only));

    let complete = concat!(
        "GEMINI MODELS\n",
        "Weekly Limit\n100% remaining\n",
        "Five Hour Limit\n99% remaining\n",
        "CLAUDE AND GPT MODELS\n",
        "Weekly Limit\nQuota available\n",
        "Five Hour Limit\nQuota available\n",
    );
    assert!(agy_usage_output_complete(complete));
}

#[test]
fn agy_startup_requires_visible_output_and_writes_only_slash_usage_once() {
    let (control_sender, control_receiver) = mpsc::channel();
    control_sender.send(b"\x1b[?1049h".to_vec()).unwrap();
    assert!(wait_for_agy_startup(&control_receiver, Duration::from_millis(2)).is_err());

    let (ready_sender, ready_receiver) = mpsc::channel();
    ready_sender
        .send(b"\x1b[?1049hAGY ready".to_vec())
        .unwrap();
    assert!(wait_for_agy_startup(&ready_receiver, Duration::from_millis(2)).is_ok());

    let mut written = Vec::new();
    write_agy_usage_command(&mut written).unwrap();
    assert_eq!(written, b"/usage\r");
}

#[test]
fn agy_probe_output_collection_stops_at_timeout_without_inventing_data() {
    let (_sender, receiver) = mpsc::channel::<Vec<u8>>();
    let output = collect_pty_probe_output(&receiver, Duration::from_millis(2));
    assert!(output.is_empty());
    assert!(parse_agy_usage_output("", TEST_NOW).is_err());
}

#[test]
fn agy_history_replaces_five_minute_buckets_and_keeps_failure_gaps() {
    let base = TEST_NOW / AGY_QUOTA_HISTORY_BUCKET_SECONDS * AGY_QUOTA_HISTORY_BUCKET_SECONDS;
    let mut history = Vec::new();
    let available = |captured_at, remaining_percent| AgentQuotaHistoryPoint {
        captured_at,
        status: "available".to_string(),
        limits: vec![AgentQuotaHistoryLimit {
            group: Some("Gemini models".to_string()),
            window_minutes: 300,
            remaining_percent: Some(remaining_percent),
        }],
    };
    let unavailable = |captured_at| AgentQuotaHistoryPoint {
        captured_at,
        status: "unavailable".to_string(),
        limits: Vec::new(),
    };

    upsert_agy_history_point(&mut history, available(base + 1, 90.0), base + 1);
    upsert_agy_history_point(&mut history, unavailable(base + 20), base + 20);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, "unavailable");
    assert!(history[0].limits.is_empty());

    upsert_agy_history_point(&mut history, available(base + 40, 88.0), base + 40);
    upsert_agy_history_point(
        &mut history,
        unavailable(base + AGY_QUOTA_HISTORY_BUCKET_SECONDS + 1),
        base + AGY_QUOTA_HISTORY_BUCKET_SECONDS + 1,
    );
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].limits[0].remaining_percent, Some(88.0));
    assert_eq!(history[1].status, "unavailable");
}

#[test]
fn agy_history_is_memory_only_bounded_and_shared_across_reconnects() {
    let histories = AgentQuotaHistories::default();
    let key = "ssh:alice@example.test:22".to_string();
    let base = TEST_NOW / AGY_QUOTA_HISTORY_BUCKET_SECONDS * AGY_QUOTA_HISTORY_BUCKET_SECONDS;
    let quota = AgentQuotaSnapshot::available(
        "agy-usage-tui",
        Some(base),
        vec![
            AgentRateLimitMetric {
                label: "five_hour".to_string(),
                group: Some("Gemini models".to_string()),
                remaining_percent: Some(91.0),
                window_minutes: Some(300),
                ..Default::default()
            },
            AgentRateLimitMetric {
                label: "weekly".to_string(),
                group: Some("Gemini models".to_string()),
                remaining_percent: Some(84.0),
                window_minutes: Some(10_080),
                ..Default::default()
            },
        ],
        base,
    );

    let mut first_connection = AgentMonitorState::default();
    first_connection.configure_agy_history(key.clone(), histories.clone());
    record_agy_history(&mut first_connection, base, Some(&quota));

    let mut reconnected = AgentMonitorState::default();
    reconnected.configure_agy_history(key, histories.clone());
    assert_eq!(reconnected.agy_history.len(), 1);
    assert_eq!(reconnected.agy_history[0].limits.len(), 2);

    let mut stored = histories.lock().unwrap();
    let history = stored.values_mut().next().unwrap();
    for index in 1..=300 {
        let captured_at = base + index * AGY_QUOTA_HISTORY_BUCKET_SECONDS;
        upsert_agy_history_point(
            history,
            AgentQuotaHistoryPoint {
                captured_at,
                status: "unavailable".to_string(),
                limits: Vec::new(),
            },
            captured_at,
        );
    }
    assert_eq!(history.len(), AGY_QUOTA_HISTORY_MAX_POINTS);
    assert!(
        history.last().unwrap().captured_at - history.first().unwrap().captured_at
            < AGY_QUOTA_HISTORY_WINDOW_SECONDS
    );
    assert!(history
        .iter()
        .all(|point| point.status == "available" || point.status == "unavailable"));
}

#[test]
fn detects_custom_claude_status_line_without_overwriting_it() {
    let settings = serde_json::json!({
        "statusLine": {
            "type": "command",
            "command": "~/.claude/my-status.sh"
        }
    });
    assert_eq!(
        claude_status_line_command(&settings).as_deref(),
        Some("~/.claude/my-status.sh")
    );
}

#[test]
fn windows_claude_setup_uses_a_shell_independent_powershell_helper() {
    let (helper_name, helper) = claude_helper_for_os(RemoteOs::Windows);
    assert_eq!(helper_name, "gputerm-claude-statusline.ps1");
    assert!(helper.contains("[Console]::In.ReadToEnd()"));
    assert!(helper.contains(".cache\\gputerm\\agent-status\\claude"));
    assert!(!helper.contains("gputerm-claude-statusline.py"));

    let command = claude_status_line_command_for(
        RemoteOs::Windows,
        r"C:\Users\Test User\.claude\gputerm-claude-statusline.ps1",
    );
    assert_eq!(
        command,
        "powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"C:/Users/Test User/.claude/gputerm-claude-statusline.ps1\""
    );
    assert!(!command.contains("%USERPROFILE%"));
    assert!(!command.contains("python"));

    let mut settings = serde_json::json!({});
    set_claude_status_line(&mut settings, &command);
    assert_eq!(
        settings.pointer("/statusLine/refreshInterval"),
        Some(&serde_json::json!(30))
    );
    assert_eq!(
        claude_status_line_command(&settings).as_deref(),
        Some(command.as_str())
    );
}

#[test]
fn windows_provider_probe_prefers_the_observed_native_executable() {
    let processes = vec![
        ProcessSample {
            name: "node.exe".to_string(),
            command: r#""C:\Program Files\nodejs\node.exe" C:\tools\codex\cli.js"#
                .to_string(),
            executable_path: Some(r"C:\Program Files\nodejs\node.exe".to_string()),
            ..Default::default()
        },
        ProcessSample {
            name: "codex.exe".to_string(),
            command: r#""C:\Users\Test User\AppData\Roaming\npm\node_modules\@openai\codex\vendor\codex.exe""#
                .to_string(),
            executable_path: Some(
                r"C:\Users\Test User\AppData\Roaming\npm\node_modules\@openai\codex\vendor\codex.exe"
                    .to_string(),
            ),
            ..Default::default()
        },
        ProcessSample {
            name: "agy.exe".to_string(),
            command: r#""C:\Users\Test User\bin\agy.exe""#.to_string(),
            executable_path: Some(r"C:\Users\Test User\bin\agy.exe".to_string()),
            ..Default::default()
        },
    ];

    assert_eq!(
        provider_executable_hint(&processes, Provider::Codex).as_deref(),
        Some(
            r"C:\Users\Test User\AppData\Roaming\npm\node_modules\@openai\codex\vendor\codex.exe"
        )
    );
    assert_eq!(
        provider_executable_hint(&processes, Provider::Agy).as_deref(),
        Some(r"C:\Users\Test User\bin\agy.exe")
    );
}

#[test]
fn macos_claude_setup_uses_the_builtin_jxa_runtime() {
    let (helper_name, helper) = claude_helper_for_os(RemoteOs::MacOs);
    assert_eq!(helper_name, "gputerm-claude-statusline-macos.js");
    assert!(helper.starts_with("#!/usr/bin/osascript -l JavaScript"));
    assert!(helper.contains("NSFileHandle.fileHandleWithStandardInput"));
    assert!(helper.contains("rate_limits"));
    assert!(!helper.contains("python"));
    assert!(local_claude_helper_runtime_available(RemoteOs::MacOs));
    assert_eq!(
        claude_status_line_command_for(RemoteOs::MacOs, "/ignored"),
        "/usr/bin/osascript -l JavaScript \"$HOME/.claude/gputerm-claude-statusline-macos.js\""
    );
}

#[cfg(target_os = "macos")]
#[test]
fn executes_macos_claude_status_line_and_writes_a_quota_snapshot() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "gputerm-claude-statusline-macos-{}-{}",
        std::process::id(),
        unique
    ));
    let session_id = format!("gputerm-macos-test-{}-{}", std::process::id(), unique);
    let script_path = root.join("gputerm-claude-statusline-macos.js");
    let snapshot_path = root
        .join(".cache")
        .join("gputerm")
        .join("agent-status")
        .join("claude")
        .join(format!("{}.json", session_id));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &script_path,
        include_str!("../../../../scripts/gputerm-claude-statusline-macos.js"),
    )
    .unwrap();

    let mut child = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript"])
        .arg(&script_path)
        .env("HOME", &root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = serde_json::json!({
        "session_id": session_id,
        "model": { "display_name": "Opus" },
        "context_window": { "used_percentage": 25 },
        "rate_limits": {
            "five_hour": { "used_percentage": 20, "resets_at": 2_000_000_000_u64 },
            "seven_day": { "used_percentage": 40, "resets_at": 2_000_100_000_u64 }
        },
        "prompt": "must not be copied"
    })
    .to_string();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let snapshot = fs::read_to_string(&snapshot_path).unwrap();
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "JXA helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Opus"));
    assert!(stdout.contains("5h 80%"));
    assert!(stdout.contains("wk 60%"));
    let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(
        snapshot
            .pointer("/rate_limits/five_hour/used_percentage")
            .and_then(Value::as_f64),
        Some(20.0)
    );
    assert!(snapshot.get("prompt").is_none());
}

#[cfg(target_os = "windows")]
#[test]
fn executes_windows_claude_status_line_and_writes_a_quota_snapshot() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "gputerm-claude-statusline-{}-{}",
        std::process::id(),
        unique
    ));
    let session_id = format!("gputerm-windows-test-{}-{}", std::process::id(), unique);
    let snapshot_path = PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join(".cache")
        .join("gputerm")
        .join("agent-status")
        .join("claude")
        .join(format!("{}.json", session_id));
    let script_path = root.join("gputerm-claude-statusline.ps1");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        &script_path,
        include_str!("../../../../scripts/gputerm-claude-statusline.ps1"),
    )
    .unwrap();

    let mut child = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = serde_json::json!({
        "session_id": session_id,
        "model": { "display_name": "Opus" },
        "context_window": { "used_percentage": 25 },
        "rate_limits": {
            "five_hour": { "used_percentage": 20, "resets_at": 2_000_000_000_u64 },
            "seven_day": { "used_percentage": 40, "resets_at": 2_000_100_000_u64 }
        }
    })
    .to_string();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let snapshot = fs::read_to_string(&snapshot_path).unwrap();
    let _ = fs::remove_file(&snapshot_path);
    let _ = fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "PowerShell helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Opus"));
    assert!(stdout.contains("5h 80%"));
    assert!(stdout.contains("wk 60%"));
    let snapshot: Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(
        snapshot
            .pointer("/rate_limits/five_hour/used_percentage")
            .and_then(Value::as_f64),
        Some(20.0)
    );
    assert_eq!(
        snapshot
            .pointer("/rate_limits/seven_day/used_percentage")
            .and_then(Value::as_f64),
        Some(40.0)
    );
}

#[test]
fn posix_claude_setup_keeps_the_executable_status_line_helper() {
    let (helper_name, helper) = claude_helper_for_os(RemoteOs::Linux);
    assert_eq!(helper_name, "gputerm-claude-statusline.sh");
    assert!(helper.starts_with("#!/bin/sh"));
    assert_eq!(
        claude_status_line_command_for(RemoteOs::Linux, "/ignored"),
        "~/.claude/gputerm-claude-statusline.sh"
    );
}

#[test]
fn local_install_is_repeatable_and_never_leaves_an_empty_helper() {
    // Reproduces the reported macOS failure: a second Set up used to
    // truncate the working helper to zero bytes, and a BOM-prefixed
    // settings.json aborted the whole install.
    let root = std::env::temp_dir().join(format!("gputerm-install-{}", uuid::Uuid::new_v4()));
    let claude_dir = root.join(".claude");
    fs::create_dir_all(&claude_dir).expect("create scratch home");
    fs::write(claude_dir.join("gputerm-claude-statusline.sh"), "").expect("stale variant");
    fs::write(
        claude_dir.join("settings.json"),
        b"\xef\xbb\xbf{\"theme\":\"dark\"}",
    )
    .expect("BOM settings");

    let os = local_os();
    let (helper_name, helper_contents) = claude_helper_for_os(os);
    let helper_path = claude_dir.join(helper_name);
    let settings_path = claude_dir.join("settings.json");

    for round in 0..2 {
        let (mut settings, existing) =
            read_claude_settings(&settings_path).expect("settings readable");
        if round == 1 {
            assert!(existing.is_some(), "the second run must see our own command");
        }
        let desired =
            claude_status_line_command_for(os, helper_path.to_string_lossy().as_ref());
        write_claude_helper(&helper_path, helper_contents).expect("helper installed");
        set_claude_status_line(&mut settings, &desired);
        backup_and_write_json(&settings_path, &settings).expect("settings written");
        remove_stale_claude_helpers(&claude_dir, helper_name);

        let written = fs::read_to_string(&helper_path).expect("helper readable");
        assert_eq!(written.len(), helper_contents.len(), "round {round}");
    }

    let settings = fs::read_to_string(&settings_path).expect("settings readable");
    assert!(settings.contains("gputerm-claude-statusline"));
    assert!(settings.contains("dark"), "unrelated keys survive");
    assert!(
        !claude_dir.join(format!("{helper_name}.gputerm-new")).exists(),
        "no temporary file is left behind"
    );
    if helper_name != "gputerm-claude-statusline.sh" {
        assert!(
            !claude_dir.join("gputerm-claude-statusline.sh").exists(),
            "the superseded variant is removed"
        );
    }

    fs::remove_dir_all(root).expect("remove scratch home");
}

#[test]
fn parses_elapsed_time_formats() {
    assert_eq!(parse_elapsed("01:30"), Some(90));
    assert_eq!(parse_elapsed("02:01:30"), Some(7290));
    assert_eq!(parse_elapsed("3-02:01:30"), Some(266_490));
}
