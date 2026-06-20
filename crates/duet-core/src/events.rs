//! Provider event streams → one normalized [`AgentEvent`].
//!
//! This module is the typed replacement for the shell version's jq/sed munging.
//! Claude Code's `--output-format stream-json` and Codex's `exec --json` emit
//! very different JSONL shapes; both are deserialized with serde into typed
//! enums and collapsed into a single provider-neutral event the renderer/TUI
//! consume. Unknown event/item kinds are caught by `#[serde(other)]` and ignored
//! rather than crashing — the compiler still forces every *known* case to be
//! handled.

use serde::Deserialize;
use serde_json::Value;

/// One provider-neutral event. Adding a third model means writing a parser that
/// produces these; the renderer and orchestrator never change.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// An assistant message addressed to the human / the other model.
    Message(String),
    /// Model "thinking" / reasoning summary.
    Reasoning(String),
    /// A structured tool invocation (Claude's tool_use).
    ToolCall { name: String, input: String },
    /// The result returned to the model from a tool.
    ToolResult(String),
    /// A shell command the agent ran (Codex command_execution).
    Command { cmdline: String, exit: Option<i64> },
    /// Files the agent edited.
    FileChange(Vec<String>),
    /// End-of-turn marker with a short usage/cost summary.
    Done(String),
}

// ───────────────────────── Claude Code stream-json ─────────────────────────

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClaudeLine {
    #[serde(rename = "assistant")]
    Assistant { message: ClaudeMsg },
    #[serde(rename = "user")]
    User { message: ClaudeMsg },
    #[serde(rename = "result")]
    Result {
        #[serde(default)]
        num_turns: i64,
        #[serde(default)]
        total_cost_usd: f64,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ClaudeMsg {
    #[serde(default)]
    content: Vec<ClaudeContent>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClaudeContent {
    #[serde(rename = "text")]
    Text {
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        #[serde(default)]
        content: Value,
    },
    #[serde(other)]
    Other,
}

/// Parse one line of Claude `stream-json` into zero or more normalized events.
pub fn parse_claude_line(line: &str) -> Vec<AgentEvent> {
    match serde_json::from_str::<ClaudeLine>(line) {
        Ok(ClaudeLine::Assistant { message }) | Ok(ClaudeLine::User { message }) => {
            message.content.into_iter().filter_map(claude_content).collect()
        }
        Ok(ClaudeLine::Result { num_turns, total_cost_usd }) => {
            vec![AgentEvent::Done(format!("turns={num_turns}  ${total_cost_usd:.3}"))]
        }
        _ => vec![],
    }
}

fn claude_content(c: ClaudeContent) -> Option<AgentEvent> {
    match c {
        ClaudeContent::Text { text } if !text.trim().is_empty() => Some(AgentEvent::Message(text)),
        ClaudeContent::ToolUse { name, input } => {
            Some(AgentEvent::ToolCall { name, input: value_to_text(&input) })
        }
        ClaudeContent::ToolResult { content } => Some(AgentEvent::ToolResult(value_to_text(&content))),
        _ => None,
    }
}

/// Pull the final assistant text out of a captured Claude stream (the `result`
/// event). Used to recover a critic's findings JSON or a plan-review markdown.
pub fn claude_final_result(raw: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct R {
        #[serde(rename = "type")]
        t: String,
        #[serde(default)]
        result: String,
    }
    raw.lines().rev().find_map(|line| {
        serde_json::from_str::<R>(line)
            .ok()
            .filter(|r| r.t == "result" && !r.result.is_empty())
            .map(|r| r.result)
    })
}

// ───────────────────────────── Codex exec --json ────────────────────────────

#[derive(Deserialize)]
#[serde(tag = "type")]
enum CodexLine {
    #[serde(rename = "item.completed")]
    Item { item: CodexItem },
    #[serde(rename = "turn.completed")]
    Turn {
        #[serde(default)]
        usage: CodexUsage,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum CodexItem {
    #[serde(rename = "agent_message")]
    Message {
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "reasoning")]
    Reasoning {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        summary: Option<Value>,
    },
    #[serde(rename = "command_execution")]
    Command {
        #[serde(default)]
        command: String,
        #[serde(default)]
        exit_code: Option<i64>,
    },
    #[serde(rename = "file_change")]
    FileChange {
        #[serde(default)]
        changes: Vec<CodexChange>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Default)]
struct CodexUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
}

#[derive(Deserialize)]
struct CodexChange {
    #[serde(default)]
    path: String,
}

/// Parse one line of Codex `exec --json` into zero or more normalized events.
pub fn parse_codex_line(line: &str) -> Vec<AgentEvent> {
    match serde_json::from_str::<CodexLine>(line) {
        Ok(CodexLine::Item { item }) => codex_item(item).into_iter().collect(),
        Ok(CodexLine::Turn { usage }) => vec![AgentEvent::Done(format!(
            "tokens in={} out={}",
            usage.input_tokens, usage.output_tokens
        ))],
        _ => vec![],
    }
}

fn codex_item(i: CodexItem) -> Option<AgentEvent> {
    match i {
        CodexItem::Message { text } if !text.trim().is_empty() => Some(AgentEvent::Message(text)),
        CodexItem::Reasoning { text, summary } => {
            let s = text.unwrap_or_else(|| summary.as_ref().map(value_to_text).unwrap_or_default());
            (!s.trim().is_empty()).then_some(AgentEvent::Reasoning(s))
        }
        CodexItem::Command { command, exit_code } => {
            Some(AgentEvent::Command { cmdline: command, exit: exit_code })
        }
        CodexItem::FileChange { changes } => {
            Some(AgentEvent::FileChange(changes.into_iter().map(|c| c.path).collect()))
        }
        _ => None,
    }
}

// ───────────────────── OpenAI-compatible Chat Completions SSE ────────────────
// Used by the local backend (LM Studio / LiteLLM / vLLM / Ollama). Kept here with
// the other parsers so it's a pure, testable `&str -> event` function.

#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    Content(String),
    Reasoning(String),
    Done,
}

/// Parse one `data: {...}` SSE frame from a streaming chat-completions response.
pub fn parse_openai_sse(line: &str) -> Option<SseEvent> {
    let data = line.trim().strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(SseEvent::Done);
    }
    let v: Value = serde_json::from_str(data).ok()?;
    let delta = v.get("choices")?.as_array()?.first()?.get("delta")?;
    if let Some(c) = delta.get("content").and_then(Value::as_str) {
        if !c.is_empty() {
            return Some(SseEvent::Content(c.to_string()));
        }
    }
    // DeepSeek-R1 / QwQ expose chain-of-thought separately
    if let Some(r) = delta.get("reasoning_content").and_then(Value::as_str) {
        if !r.is_empty() {
            return Some(SseEvent::Reasoning(r.to_string()));
        }
    }
    None
}

/// Strip markdown code fences (shared by critic backends to recover JSON a model
/// wrapped in ```fences```).
pub fn strip_fences(s: &str) -> String {
    s.lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Recover a JSON object from a model reply: drop ``` fences, then take the
/// outermost `{ ... }` if present. Shared by every JSON-parsing path (the
/// orchestrated critic and `duet local-review`) so identical model output parses
/// the same way everywhere.
pub fn extract_json(s: &str) -> String {
    let no_fence = strip_fences(s);
    match (no_fence.find('{'), no_fence.rfind('}')) {
        (Some(a), Some(b)) if b >= a => no_fence[a..=b].to_string(),
        _ => no_fence,
    }
}

/// Adapt a local backend's saved SSE line to normalized events (for replay).
pub fn parse_local_line(line: &str) -> Vec<AgentEvent> {
    match parse_openai_sse(line) {
        Some(SseEvent::Content(c)) => vec![AgentEvent::Message(c)],
        Some(SseEvent::Reasoning(r)) => vec![AgentEvent::Reasoning(r)],
        _ => vec![],
    }
}

/// Render a JSON value as compact human text: a string as-is, an array of
/// `{text: ...}` objects joined, otherwise compact JSON.
fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Array(items) => items
            .iter()
            .map(|it| {
                it.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| it.to_string())
            })
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_text_and_result() {
        let text = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"PONG"}]}}"#;
        assert_eq!(parse_claude_line(text), vec![AgentEvent::Message("PONG".into())]);
        let res = r#"{"type":"result","subtype":"success","result":"PONG","num_turns":1,"total_cost_usd":0.1}"#;
        assert_eq!(parse_claude_line(res), vec![AgentEvent::Done("turns=1  $0.100".into())]);
    }

    #[test]
    fn claude_tool_use_and_unknown() {
        let tu = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.py"}}]}}"#;
        assert_eq!(
            parse_claude_line(tu),
            vec![AgentEvent::ToolCall { name: "Edit".into(), input: "{\"file_path\":\"a.py\"}".into() }]
        );
        // system / stream_event lines are ignored, not errors
        assert!(parse_claude_line(r#"{"type":"system","subtype":"init"}"#).is_empty());
        assert!(parse_claude_line("not json").is_empty());
    }

    #[test]
    fn codex_message_command_turn() {
        let msg = r#"{"type":"item.completed","item":{"id":"i0","type":"agent_message","text":"hi"}}"#;
        assert_eq!(parse_codex_line(msg), vec![AgentEvent::Message("hi".into())]);
        let cmd = r#"{"type":"item.completed","item":{"type":"command_execution","command":"git status","exit_code":0}}"#;
        assert_eq!(
            parse_codex_line(cmd),
            vec![AgentEvent::Command { cmdline: "git status".into(), exit: Some(0) }]
        );
        let turn = r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":2}}"#;
        assert_eq!(parse_codex_line(turn), vec![AgentEvent::Done("tokens in=10 out=2".into())]);
        assert!(parse_codex_line(r#"{"type":"thread.started","thread_id":"x"}"#).is_empty());
    }

    #[test]
    fn final_result_extraction() {
        let raw = "{\"type\":\"system\"}\n{\"type\":\"result\",\"result\":\"the answer\"}\n";
        assert_eq!(claude_final_result(raw).as_deref(), Some("the answer"));
    }

    #[test]
    fn openai_sse_frames() {
        let c = r#"data: {"choices":[{"delta":{"content":"hel"}}]}"#;
        assert_eq!(parse_openai_sse(c), Some(SseEvent::Content("hel".into())));
        let r = r#"data: {"choices":[{"delta":{"reasoning_content":"let me think"}}]}"#;
        assert_eq!(parse_openai_sse(r), Some(SseEvent::Reasoning("let me think".into())));
        assert_eq!(parse_openai_sse("data: [DONE]"), Some(SseEvent::Done));
        // role-only opening delta and keepalives produce nothing
        assert_eq!(parse_openai_sse(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#), None);
        assert_eq!(parse_openai_sse(": keepalive"), None);
    }
}
