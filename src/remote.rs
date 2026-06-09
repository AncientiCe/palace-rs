//! Remote MCP proxy transport.
//!
//! When palace runs in remote mode (`mcp_mode = "remote"`), `palace mcp` becomes a
//! transparent stdio→HTTP bridge: each JSON-RPC request from the AI client is forwarded to
//! the remote palace-server `/mcp` endpoint with a Bearer API key, and the response is
//! written back to stdout verbatim. This keeps the stdio registration in every AI client
//! unchanged while routing memory to the shared remote server.

use anyhow::{Context, Result};
use std::io::{self, BufRead, Write};
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{json, Value};
use tracing::{error, info};

/// Build the blocking HTTP client used for forwarding.
fn build_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("failed to build HTTP client for remote MCP proxy")
}

/// Run the stdio→HTTP proxy loop against the remote palace-server `/mcp` endpoint.
pub fn run(url: &str, api_key: &str) -> Result<()> {
    let client = build_client()?;
    info!(endpoint = %url, "Palace MCP server starting in remote (proxy) mode");

    let stdin = io::stdin();
    let stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        // Parse enough to recover the request id and detect notifications (no `id`).
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                write_line(
                    &stdout,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": {"code": -32700, "message": format!("Parse error: {e}")}
                    }),
                )?;
                continue;
            }
        };

        let req_id = request.get("id").cloned();
        let is_notification = req_id.is_none();
        let method = request
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        match forward(&client, url, api_key, &line) {
            Ok(Some(response)) if !is_notification => {
                write_line(&stdout, &adapt_response(&method, response))?
            }
            Ok(_) => {} // notification, or empty body: nothing to write back
            Err(e) => {
                error!(error = %e, "remote MCP forward failed");
                if !is_notification {
                    write_line(
                        &stdout,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": req_id.unwrap_or(Value::Null),
                            "error": {
                                "code": -32000,
                                "message": format!("Remote palace-server error: {e}")
                            }
                        }),
                    )?;
                }
            }
        }
    }

    Ok(())
}

/// Forward one raw JSON-RPC request body to the remote server and return the parsed
/// response (if any). Handles both `application/json` and `text/event-stream` responses.
fn forward(client: &Client, url: &str, api_key: &str, body: &str) -> Result<Option<Value>> {
    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(body.to_string())
        .send()
        .context("request to remote palace-server failed")?;

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let text = resp.text().context("failed to read remote response body")?;

    if !status.is_success() {
        let detail = text.trim();
        if status.as_u16() == 401 {
            anyhow::bail!("HTTP 401 Unauthorized — check the API key (`palace remote set`)");
        }
        anyhow::bail!("HTTP {}: {}", status.as_u16(), detail);
    }

    if text.trim().is_empty() {
        return Ok(None);
    }

    if content_type.contains("text/event-stream") {
        Ok(parse_sse(&text))
    } else {
        let value: Value =
            serde_json::from_str(text.trim()).context("invalid JSON from remote palace-server")?;
        Ok(Some(value))
    }
}

/// Adapt a palace-server response into spec-compliant MCP shape.
///
/// palace-server wraps **every** `Ok` result — including `initialize` and `tools/list` —
/// in `{"result": {"content": [{"type":"text","text": "<stringified json>"}]}}`. That is
/// correct for `tools/call` (tool output is a content array) but non-standard for
/// `initialize` (client expects `result.capabilities`/`result.protocolVersion`) and
/// `tools/list` (client expects `result.tools`). For those two methods we hoist the inner
/// JSON out of the text envelope so any standard MCP client understands the response.
///
/// Defensive: only rewrites when the envelope is actually present and its text parses as
/// JSON, so a spec-compliant server (or a future fixed palace-server) passes through
/// unchanged.
fn adapt_response(method: &str, mut resp: Value) -> Value {
    if method != "initialize" && method != "tools/list" {
        return resp;
    }
    let inner = resp
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c0| c0.get("text"))
        .and_then(|t| t.as_str())
        .and_then(|s| serde_json::from_str::<Value>(s).ok());
    if let Some(inner) = inner {
        if let Some(obj) = resp.as_object_mut() {
            obj.insert("result".to_string(), inner);
        }
    }
    resp
}

/// Extract the last JSON-RPC message from an SSE stream body (the response to our request).
fn parse_sse(body: &str) -> Option<Value> {
    let mut last = None;
    for line in body.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                last = Some(v);
            }
        }
    }
    last
}

fn write_line(stdout: &io::Stdout, value: &Value) -> Result<()> {
    let mut out = stdout.lock();
    writeln!(out, "{value}")?;
    out.flush()?;
    Ok(())
}

/// One-shot connectivity + auth probe used by `palace remote test`.
/// Returns the number of tools reported by the remote on success.
pub fn probe(url: &str, api_key: &str) -> Result<usize> {
    let client = build_client()?;

    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "palace-cli", "version": env!("CARGO_PKG_VERSION")}
        }
    });
    forward(&client, url, api_key, &init.to_string()).context("initialize handshake failed")?;

    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    let resp = forward(&client, url, api_key, &list.to_string())
        .context("tools/list failed")?
        .ok_or_else(|| anyhow::anyhow!("empty tools/list response from remote"))?;
    let resp = adapt_response("tools/list", resp);

    if let Some(err) = resp.get("error") {
        anyhow::bail!("remote returned error: {err}");
    }

    let count = resp
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_returns_last_data_payload() {
        let body =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        let v = parse_sse(body).expect("parsed");
        assert_eq!(v["result"]["ok"], serde_json::json!(true));
    }

    #[test]
    fn sse_skips_done_and_blank() {
        let body = "data: \ndata: [DONE]\n";
        assert!(parse_sse(body).is_none());
    }

    // Real palace-server envelopes captured from a live server.
    #[test]
    fn adapt_unwraps_initialize_envelope() {
        let raw = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"content": [{"type": "text",
                "text": "{\"capabilities\":{\"tools\":{\"listChanged\":false}},\"protocolVersion\":\"2024-11-05\",\"serverInfo\":{\"name\":\"palace-server\",\"version\":\"0.3.0\"}}"}]}
        });
        let out = adapt_response("initialize", raw);
        assert_eq!(out["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(out["result"]["serverInfo"]["name"], "palace-server");
        // Envelope is gone.
        assert!(out["result"].get("content").is_none());
    }

    #[test]
    fn adapt_unwraps_tools_list_envelope() {
        let raw = serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "result": {"content": [{"type": "text",
                "text": "{\"tools\":[{\"name\":\"palace_status\"},{\"name\":\"palace_search\"}]}"}]}
        });
        let out = adapt_response("tools/list", raw);
        let tools = out["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "palace_status");
    }

    #[test]
    fn adapt_leaves_tools_call_unchanged() {
        // tools/call already uses the standard content array — must pass through verbatim.
        let raw = serde_json::json!({
            "jsonrpc": "2.0", "id": 3,
            "result": {"content": [{"type": "text", "text": "{\"licensed\":true}"}]}
        });
        let out = adapt_response("tools/call", raw.clone());
        assert_eq!(out, raw);
    }

    #[test]
    fn adapt_passes_through_compliant_server() {
        // A spec-compliant server returns result.tools directly — leave it alone.
        let raw = serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "result": {"tools": [{"name": "palace_status"}]}
        });
        let out = adapt_response("tools/list", raw.clone());
        assert_eq!(out, raw);
    }

    #[test]
    fn adapt_passes_through_error_response() {
        let raw = serde_json::json!({
            "jsonrpc": "2.0", "id": 2,
            "error": {"code": -32001, "message": "license inactive"}
        });
        let out = adapt_response("tools/list", raw.clone());
        assert_eq!(out, raw);
    }
}
