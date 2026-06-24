//! Integration tests for the local OpenAI-compatible backend.
//!
//! They stand up a tiny in-process HTTP server (std `TcpListener`, no async and
//! no extra dependencies, matching the crate's blocking `ureq` design) and assert
//! the exact request `LocalBackend` sends and how it parses the reply. This pins
//! the ureq client behaviour that is otherwise only exercised against a live
//! LM Studio / LiteLLM server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use duet_agents::LocalBackend;
use duet_core::events::AgentEvent;
use duet_core::report::{ChannelReporter, UiMsg};

/// What the mock server received.
struct Captured {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A one-shot HTTP server: replies to the first request with `status` / `ctype` /
/// `resp_body`, then returns the request it captured. Binds an ephemeral port and
/// returns its base URL (no trailing slash, as `LocalBackend` expects).
fn one_shot(status: &str, ctype: &str, resp_body: String) -> (String, thread::JoinHandle<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let status = status.to_string();
    let ctype = ctype.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

        // Request line: "<METHOD> <PATH> HTTP/1.1"
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();

        // Headers until the blank line.
        let mut headers: Vec<(String, String)> = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let (k, v) = (k.trim().to_string(), v.trim().to_string());
                if k.eq_ignore_ascii_case("content-length") {
                    content_length = v.parse().unwrap_or(0);
                }
                headers.push((k, v));
            }
        }

        // Body (exactly Content-Length bytes, if any).
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body).unwrap();
        }

        let clen = resp_body.len();
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {clen}\r\nConnection: close\r\n\r\n{resp_body}"
        );
        stream.write_all(resp.as_bytes()).unwrap();
        stream.flush().unwrap();

        Captured {
            method,
            path,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
        }
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

/// Encode one OpenAI-style streaming chunk carrying `content`.
fn sse_chunk(content: &str) -> String {
    let v = serde_json::json!({ "choices": [{ "delta": { "content": content } }] });
    format!("data: {v}\n\n")
}

#[test]
fn list_models_parses_served_ids() {
    let (base, handle) = one_shot(
        "200 OK",
        "application/json",
        r#"{"object":"list","data":[{"id":"alpha"},{"id":"beta"}]}"#.to_string(),
    );

    let models = LocalBackend::list_models(&base, 5).expect("list_models");
    let req = handle.join().unwrap();

    assert_eq!(req.method, "GET");
    assert_eq!(req.path, "/models");
    assert_eq!(models, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn critique_posts_expected_request_and_assembles_stream() {
    // Two content deltas that concatenate to a JSON object, then [DONE].
    let sse = format!(
        "{}{}data: [DONE]\n\n",
        sse_chunk("{\"verdict\":\"approve\","),
        sse_chunk("\"findings\":[]}"),
    );
    let (base, handle) = one_shot("200 OK", "text/event-stream", sse);

    let backend = LocalBackend::new(&base, "test-model", Some("test-key".to_string()), 5);
    let (tx, rx) = std::sync::mpsc::channel();
    let reporter = ChannelReporter { tx };
    let raw = std::env::temp_dir().join("duet-local-backend-test.sse.jsonl");

    let content = backend
        .critique("system prompt", "user prompt", None, &raw, &reporter)
        .expect("critique");

    let req = handle.join().unwrap();

    // Request shape — this is what the ureq 3 migration had to get right.
    assert_eq!(req.method, "POST");
    assert_eq!(req.path, "/chat/completions");
    assert!(
        req.header("content-type")
            .is_some_and(|c| c.starts_with("application/json")),
        "send_json must set a JSON Content-Type, got {:?}",
        req.header("content-type")
    );
    assert_eq!(req.header("authorization"), Some("Bearer test-key"));

    let body: serde_json::Value = serde_json::from_str(&req.body).expect("request body is JSON");
    assert_eq!(body["model"], "test-model");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "system prompt");
    assert_eq!(body["messages"][1]["role"], "user");

    // The streamed deltas are reassembled into the full assistant content.
    assert_eq!(content, "{\"verdict\":\"approve\",\"findings\":[]}");

    // And the reporter saw the streamed line.
    let saw = rx
        .try_iter()
        .any(|m| matches!(m, UiMsg::Event(_, AgentEvent::Message(s)) if s.contains("verdict")));
    assert!(saw, "reporter should receive the streamed message line");
}

#[test]
fn critique_omits_authorization_when_no_api_key() {
    let sse = format!("{}data: [DONE]\n\n", sse_chunk("ok"));
    let (base, handle) = one_shot("200 OK", "text/event-stream", sse);

    let backend = LocalBackend::new(&base, "m", None, 5);
    let (tx, _rx) = std::sync::mpsc::channel();
    let reporter = ChannelReporter { tx };
    let raw = std::env::temp_dir().join("duet-local-backend-noauth.sse.jsonl");

    let content = backend
        .critique("s", "u", None, &raw, &reporter)
        .expect("critique");
    let req = handle.join().unwrap();

    assert_eq!(content, "ok");
    assert_eq!(req.header("authorization"), None, "no api key → no Authorization header");
}
