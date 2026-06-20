//! Integration tests that run the typed parsers over the EXACT JSONL the real
//! Claude/Codex CLIs produced during shell-version testing. This is the payoff
//! of the Rust port: the event layer is verified against ground-truth fixtures,
//! offline, with zero API calls.

use duet_core::events::{parse_claude_line, parse_codex_line, AgentEvent};

#[test]
fn real_claude_build_stream_has_tools_messages_and_one_done() {
    let data = include_str!("fixtures/claude_build.jsonl");
    let (mut tools, mut messages, mut done) = (0, 0, 0);
    for line in data.lines() {
        for ev in parse_claude_line(line) {
            match ev {
                AgentEvent::ToolCall { .. } => tools += 1,
                AgentEvent::Message(_) => messages += 1,
                AgentEvent::Done(_) => done += 1,
                _ => {}
            }
        }
    }
    assert!(tools > 0, "expected tool calls in the build stream");
    assert!(messages > 0, "expected assistant messages");
    assert_eq!(done, 1, "expected exactly one result/done event");
}

#[test]
fn real_codex_review_stream_has_commands_and_message() {
    let data = include_str!("fixtures/codex_review.jsonl");
    let (mut commands, mut messages) = (0, 0);
    for line in data.lines() {
        for ev in parse_codex_line(line) {
            match ev {
                AgentEvent::Command { .. } => commands += 1,
                AgentEvent::Message(_) => messages += 1,
                _ => {}
            }
        }
    }
    assert!(commands > 0, "codex review should have run commands");
    assert!(messages > 0, "codex should emit a final message");
}

#[test]
fn every_fixture_line_parses_without_panic() {
    for data in [
        include_str!("fixtures/claude_simple.jsonl"),
        include_str!("fixtures/codex_simple.jsonl"),
        include_str!("fixtures/claude_build.jsonl"),
        include_str!("fixtures/codex_review.jsonl"),
    ] {
        for line in data.lines() {
            let _ = parse_claude_line(line);
            let _ = parse_codex_line(line);
        }
    }
}
