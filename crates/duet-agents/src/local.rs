//! Local / OpenAI-compatible backend (LM Studio, LiteLLM, vLLM, Ollama) over
//! HTTP via the blocking `ureq` client — no async runtime, matching the rest of
//! the codebase. It is a **critic**: a plain chat endpoint has no file/exec
//! tools, so the reviewable artifact (a diff, a document) is inlined into the
//! prompt by the caller, and the model returns a structured critique. It cannot
//! be a builder — that needs an autonomous edit/exec loop a chat call lacks.

use duet_core::events::{parse_openai_sse, AgentEvent, SseEvent};
use duet_core::render::Model;
use duet_core::report::Reporter;
use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

pub fn default_endpoint() -> String {
    std::env::var("DUET_LOCAL_BASE_URL").unwrap_or_else(|_| "http://localhost:1234/v1".to_string())
}

pub struct LocalBackend {
    endpoint: String,
    model: String,
    api_key: Option<String>,
    agent: ureq::Agent,
}

impl LocalBackend {
    pub fn new(endpoint: &str, model: &str, api_key: Option<String>, timeout_secs: u64) -> Self {
        let agent = Self::agent(timeout_secs);
        LocalBackend { endpoint: endpoint.trim_end_matches('/').to_string(), model: model.to_string(), api_key, agent }
    }

    /// A blocking agent with an overall per-request deadline.
    fn agent(timeout_secs: u64) -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(timeout_secs)))
            .build();
        ureq::Agent::new_with_config(config)
    }

    /// GET /models — the served model ids (local analog of `codex login status`).
    pub fn list_models(endpoint: &str, timeout_secs: u64) -> Result<Vec<String>> {
        let agent = Self::agent(timeout_secs);
        let url = format!("{}/models", endpoint.trim_end_matches('/'));
        let reader = agent.get(&url).call().map_err(|e| anyhow!("{e}"))?.into_body().into_reader();
        let v: serde_json::Value = serde_json::from_reader(reader)?;
        Ok(v.get("data")
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from)).collect())
            .unwrap_or_default())
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Stream a critique. `system`/`user` are the framed prompt (the artifact must
    /// already be inlined into `user`). `schema` requests JSON-schema-constrained
    /// output when the server supports it (falls back gracefully). Returns the full
    /// assistant content (the JSON/markdown) for the caller to parse.
    pub fn critique(&self, system: &str, user: &str, schema: Option<&str>, raw: &Path, reporter: &dyn Reporter) -> Result<String> {
        let url = format!("{}/chat/completions", self.endpoint);
        let mut body = serde_json::json!({
            "model": self.model,
            "stream": true,
            "temperature": 0.2,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        if let Some(s) = schema {
            if let Ok(schema_val) = serde_json::from_str::<serde_json::Value>(s) {
                body["response_format"] = serde_json::json!({
                    "type": "json_schema",
                    "json_schema": { "name": "critique", "strict": true, "schema": schema_val }
                });
            }
        }

        // Try with structured output; on any HTTP error, retry without response_format.
        let resp = match self.post(&url, &body) {
            Ok(r) => r,
            Err(_) => {
                if let Some(obj) = body.as_object_mut() {
                    obj.remove("response_format");
                }
                self.post(&url, &body)?
            }
        };

        let mut rawf = std::fs::File::create(raw).ok();
        let mut content = String::new();
        let mut line_buf = String::new();
        for line in BufReader::new(resp.into_body().into_reader()).lines() {
            let line = line?;
            if let Some(f) = rawf.as_mut() {
                let _ = writeln!(f, "{line}");
            }
            match parse_openai_sse(&line) {
                Some(SseEvent::Content(c)) => {
                    content.push_str(&c);
                    line_buf.push_str(&c);
                    // emit complete lines so the live view shows lines, not tokens
                    while let Some(nl) = line_buf.find('\n') {
                        let l: String = line_buf.drain(..=nl).collect();
                        let l = l.trim_end();
                        if !l.is_empty() {
                            reporter.event(Model::Local, &AgentEvent::Message(l.to_string()));
                        }
                    }
                }
                Some(SseEvent::Reasoning(_)) => { /* chain-of-thought: accumulated by the server, not shown */ }
                Some(SseEvent::Done) => break,
                None => {}
            }
        }
        let tail = line_buf.trim();
        if !tail.is_empty() {
            reporter.event(Model::Local, &AgentEvent::Message(tail.to_string()));
        }
        reporter.event(Model::Local, &AgentEvent::Done(format!("{} chars", content.len())));
        Ok(content)
    }

    fn post(&self, url: &str, body: &serde_json::Value) -> Result<ureq::http::Response<ureq::Body>> {
        // `send_json` sets `Content-Type: application/json` for us.
        let mut req = self.agent.post(url);
        if let Some(k) = &self.api_key {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        req.send_json(body).map_err(|e| anyhow!("POST {url}: {e}"))
    }
}
