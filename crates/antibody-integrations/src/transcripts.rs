//! Normalize agent transcripts into `TranscriptMessage`s.
//!
//! Claude Code and Codex store transcripts as internal JSONL formats that can
//! change between versions, so parsing is lenient: unknown lines are skipped
//! and nothing here fails on unexpected shapes.
//!
//! Bounds keep payloads small: tool output keeps its head and tail (errors
//! usually appear at the end), reasoning/thinking blocks and images are
//! dropped, and long sessions keep their beginning and end.

use antibody_core::api::TranscriptMessage;
use antibody_core::model::MessageRole;
use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_messages: usize,
    pub max_message_chars: usize,
    pub max_tool_chars: usize,
    pub max_total_chars: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_messages: 400,
            max_message_chars: 6000,
            max_tool_chars: 2500,
            max_total_chars: 900_000,
        }
    }
}

fn head_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let head = max / 3;
    let tail = max - head - 20;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(count - tail).collect();
    format!(
        "{start}\n[… {} chars omitted …]\n{end}",
        count - head - tail
    )
}

fn push(
    out: &mut Vec<TranscriptMessage>,
    role: MessageRole,
    content: String,
    tool_name: Option<String>,
    limits: &Limits,
) {
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    let max = if role == MessageRole::Tool {
        limits.max_tool_chars
    } else {
        limits.max_message_chars
    };
    out.push(TranscriptMessage {
        role,
        content: head_tail(content, max),
        tool_name,
    });
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| match b {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => match o.get("type").and_then(Value::as_str) {
                    Some("text") | Some("input_text") | Some("output_text") => {
                        o.get("text").and_then(Value::as_str).map(str::to_string)
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn summarize_tool_input(name: &str, input: &Value) -> String {
    // Keep commands and file paths (useful context), not file contents.
    let mut parts = Vec::new();
    if let Some(o) = input.as_object() {
        for key in [
            "command",
            "cmd",
            "file_path",
            "path",
            "pattern",
            "description",
        ] {
            if let Some(v) = o.get(key) {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                parts.push(format!(
                    "{key}: {}",
                    s.chars().take(400).collect::<String>()
                ));
            }
        }
    } else if let Some(s) = input.as_str() {
        parts.push(s.chars().take(400).collect());
    }
    format!("[tool call {name}] {}", parts.join(" | "))
}

/// Parse a Claude Code session JSONL transcript.
pub fn parse_claude_code(jsonl: &str, limits: &Limits) -> Vec<TranscriptMessage> {
    let mut out = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        if ty != "user" && ty != "assistant" {
            continue;
        }
        if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(message) = v.get("message") else {
            continue;
        };
        let content = message.get("content").cloned().unwrap_or(Value::Null);
        match content {
            Value::String(s) => {
                let role = if ty == "user" {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                };
                push(&mut out, role, s, None, limits);
            }
            Value::Array(blocks) => {
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let role = if ty == "user" {
                                MessageRole::User
                            } else {
                                MessageRole::Assistant
                            };
                            push(
                                &mut out,
                                role,
                                b.get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                                None,
                                limits,
                            );
                        }
                        Some("tool_use") => {
                            let name = b
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_string();
                            let summary =
                                summarize_tool_input(&name, b.get("input").unwrap_or(&Value::Null));
                            push(
                                &mut out,
                                MessageRole::Assistant,
                                summary,
                                Some(name),
                                limits,
                            );
                        }
                        Some("tool_result") => {
                            let text = text_of(b.get("content").unwrap_or(&Value::Null));
                            let is_error = b.get("is_error").and_then(Value::as_bool) == Some(true);
                            let text = if is_error {
                                format!("[tool error]\n{text}")
                            } else {
                                text
                            };
                            push(&mut out, MessageRole::Tool, text, None, limits);
                        }
                        _ => {} // thinking, images, etc.
                    }
                }
            }
            _ => {}
        }
    }
    bound(out, limits)
}

/// Parse a Codex rollout JSONL transcript.
pub fn parse_codex(jsonl: &str, limits: &Limits) -> Vec<TranscriptMessage> {
    let mut out = Vec::new();
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let Some(p) = v.get("payload") else { continue };
        match p.get("type").and_then(Value::as_str) {
            Some("message") => {
                let role = match p.get("role").and_then(Value::as_str) {
                    Some("user") => MessageRole::User,
                    Some("assistant") => MessageRole::Assistant,
                    _ => continue, // developer/system prompts are not session content
                };
                let text = text_of(p.get("content").unwrap_or(&Value::Null));
                // Codex injects environment context as user messages.
                if role == MessageRole::User
                    && text.trim_start().starts_with('<')
                    && text.contains("environment_context")
                {
                    continue;
                }
                push(&mut out, role, text, None, limits);
            }
            Some("agent_message") => {
                push(
                    &mut out,
                    MessageRole::Assistant,
                    text_of(p.get("content").unwrap_or(&Value::Null)),
                    None,
                    limits,
                );
            }
            Some("function_call") | Some("custom_tool_call") | Some("local_shell_call") => {
                let name = p
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let input = p
                    .get("arguments")
                    .and_then(Value::as_str)
                    .and_then(|a| serde_json::from_str::<Value>(a).ok())
                    .or_else(|| p.get("input").cloned())
                    .or_else(|| p.get("action").cloned())
                    .unwrap_or(Value::Null);
                push(
                    &mut out,
                    MessageRole::Assistant,
                    summarize_tool_input(&name, &input),
                    Some(name),
                    limits,
                );
            }
            Some("function_call_output")
            | Some("custom_tool_call_output")
            | Some("local_shell_call_output") => {
                let output = p.get("output").cloned().unwrap_or(Value::Null);
                let text = match &output {
                    Value::String(s) => serde_json::from_str::<Value>(s)
                        .ok()
                        .and_then(|j| j.get("output").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_else(|| s.clone()),
                    other => text_of(other),
                };
                push(&mut out, MessageRole::Tool, text, None, limits);
            }
            _ => {}
        }
    }
    bound(out, limits)
}

/// Keep the first quarter and the most recent messages within limits.
fn bound(mut msgs: Vec<TranscriptMessage>, limits: &Limits) -> Vec<TranscriptMessage> {
    if msgs.len() > limits.max_messages {
        let head = limits.max_messages / 4;
        let tail = limits.max_messages - head - 1;
        let omitted = msgs.len() - head - tail;
        let mut kept: Vec<TranscriptMessage> = msgs.drain(..head).collect();
        kept.push(TranscriptMessage {
            role: MessageRole::System,
            content: format!("[{omitted} messages omitted]"),
            tool_name: None,
        });
        let start = msgs.len() - tail;
        kept.extend(msgs.drain(start..));
        msgs = kept;
    }
    let mut total: usize = msgs.iter().map(|m| m.content.len()).sum();
    while total > limits.max_total_chars && msgs.len() > 2 {
        let removed = msgs.remove(msgs.len() / 2);
        total -= removed.content.len();
    }
    msgs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_claude_code_lines() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"tests fail with pool timeout"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"secret"},{"type":"text","text":"Let me run the tests."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":"error: pool timed out"}]}}"#,
            r#"{"type":"attachment","foo":1}"#,
            "not json",
        ]
        .join("\n");
        let msgs = parse_claude_code(&lines, &Limits::default());
        assert_eq!(msgs.len(), 4, "{msgs:?}");
        assert_eq!(msgs[0].role, MessageRole::User);
        assert!(msgs[2].content.contains("cargo test"));
        assert_eq!(msgs[3].role, MessageRole::Tool);
        assert!(msgs[3].content.contains("[tool error]"));
        assert!(!msgs.iter().any(|m| m.content.contains("secret")));
    }

    #[test]
    fn parses_codex_lines() {
        let lines = [
            r#"{"type":"session_meta","payload":{"id":"x"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"system stuff"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix the build"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"cargo\",\"build\"]}"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","output":"{\"output\":\"error[E0432]: unresolved import\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Fixed the import."}]}}"#,
        ]
        .join("\n");
        let msgs = parse_codex(&lines, &Limits::default());
        assert_eq!(msgs.len(), 4, "{msgs:?}");
        assert!(msgs[1].content.contains("cargo"));
        assert_eq!(msgs[2].content, "error[E0432]: unresolved import");
        assert_eq!(msgs[3].role, MessageRole::Assistant);
    }

    #[test]
    fn bounds_long_sessions_and_tool_output() {
        let limits = Limits {
            max_messages: 10,
            max_message_chars: 50,
            max_tool_chars: 40,
            max_total_chars: 10_000,
        };
        let lines: Vec<String> = (0..30)
            .map(|i| {
                format!(
                    r#"{{"type":"user","message":{{"role":"user","content":"message {i} {}"}}}}"#,
                    "x".repeat(100)
                )
            })
            .collect();
        let msgs = parse_claude_code(&lines.join("\n"), &limits);
        assert_eq!(msgs.len(), 10);
        assert!(msgs[0].content.starts_with("message 0"));
        assert!(msgs.iter().any(|m| m.content.contains("omitted")));
        assert!(msgs.last().unwrap().content.starts_with("message 29"));
        assert!(msgs[0].content.chars().count() < 120);
    }
}
