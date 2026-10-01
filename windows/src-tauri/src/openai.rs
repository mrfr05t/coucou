// OpenAI-compatible chat transport.
//
// Two routes, in order of preference:
//
//   POST {base_url}/responses        the Responses API — web search tool and
//                                    PDF input, which is what api.openai.com
//                                    itself wants.
//   POST {base_url}/chat/completions Chat Completions — what almost every
//                                    "OpenAI-compatible" server speaks
//                                    (Ollama, LM Studio, OpenRouter, vLLM…).
//
// The ladder is climbed once per base URL and remembered, so a server that
// only speaks Chat Completions costs one extra 404 on the first turn and
// nothing afterwards. The route that worked also decides whether web search
// is on: it is a Responses tool, so on Chat Completions the island answers
// from the model rather than failing.
//
// The key never leaves the Credential Manager and file bytes never cross the
// IPC boundary — same rule as `claude.rs`.

use std::sync::Mutex;

use serde_json::{json, Value};

use crate::chat::{self, api_error, Chat, ChatContext, ChatReply, FileContent, SYSTEM_PROMPT};
use crate::secrets;

pub const DEFAULT_MODEL: &str = "gpt-5";
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const MAX_TOKENS: u32 = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Responses,
    Chat,
}

/// What turned out to work for one base URL: which route exists, whether the
/// server accepts our web search tool, and whether it can take attachments at
/// all. llama.cpp answers `/responses` but refuses `input_file`, and answers
/// images with a 500 when the model has no projector — each of those is worth
/// one downgrade, not a dead end in the island.
#[derive(Clone, Copy)]
struct Policy {
    endpoint: Endpoint,
    search: bool,
    media: bool,
}

/// What each base URL turned out to accept, so a new base URL in settings is
/// free to be a different server entirely — and coming back remembers.
/// One entry per URL rather than a single slot: nothing here is hot enough to
/// justify the ambiguity.
static POLICY: Mutex<Vec<(String, Policy)>> = Mutex::new(Vec::new());

/// One chat turn. Returns the assistant's text, or a message the island shows
/// in the note view.
pub async fn send(
    chat: &Chat,
    base_url: &str,
    model: &str,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    chat.ensure_provider("openai");

    // A local server — Ollama, LM Studio — has no key to check, so a missing
    // secret is not an error here. api.openai.com answers 401 with its own
    // message, which is a better error than refusing to try at all.
    let key = secrets::get("openai-api-key");
    let base = normalise_base(base_url);

    // File / window context rides along with the first message only, exactly
    // like ClaudeService.chat().
    let mut parts: Vec<Value> = Vec::new();
    if chat.is_empty() {
        if let Some(mut extra) = context_parts(&context) {
            parts.append(&mut extra);
        }
    }
    parts.push(text_part(&query));
    chat.push(json!({ "role": "user", "parts": parts }));

    let text = match turn(chat, &base, key.as_deref(), model).await {
        Ok(text) => text,
        Err(err) => {
            chat.pop(); // keep the history consistent with what the model saw
            return Err(err);
        }
    };

    chat.push(json!({ "role": "assistant", "parts": [{ "kind": "text", "text": text }] }));
    Ok(ChatReply { text })
}

/// Climb the ladder at most three steps deep — route, then web search, then
/// attachments — so a server that only half-implements the API still ends up
/// answering instead of showing the island an error.
async fn turn(chat: &Chat, base: &str, key: Option<&str>, model: &str) -> Result<String, String> {
    let mut policy = policy_for(base);
    let history = chat.snapshot();

    for _ in 0..4 {
        let (url, body) = build(base, &policy, model, &history);
        let response = post(&url, key, &body).await?;

        if response.status.is_success() {
            return extract(&policy, &response.body);
        }

        let detail = api_error(&response.body);
        let status = response.status.as_u16();
        let route_missing = matches!(status, 404 | 405 | 501);
        let search_rejected =
            (400..500).contains(&status) && mentions(&detail, &["web_search", "\"tools\"", "unknown parameter"]);
        let media_rejected = mentions(&detail, &["input_file", "input_image", "image input", "mmproj"]);

        if route_missing && policy.endpoint == Endpoint::Responses {
            policy.endpoint = Endpoint::Chat;
            remember(base, policy);
            continue;
        }
        if search_rejected && policy.search {
            policy.search = false;
            remember(base, policy);
            continue;
        }
        if media_rejected && policy.media && has_media(&history) {
            policy.media = false;
            remember(base, policy);
            continue;
        }
        // Surface the server's own message, which is what makes a bad key or
        // a bad model name obvious.
        return Err(format!("OpenAI API {status}: {detail}"));
    }
    Err("OpenAI API: this server offers neither /responses nor /chat/completions.".into())
}

fn policy_for(base: &str) -> Policy {
    POLICY
        .lock()
        .ok()
        .and_then(|guard| guard.iter().find(|(b, _)| b == base).map(|(_, p)| *p))
        .unwrap_or(Policy { endpoint: Endpoint::Responses, search: true, media: true })
}

fn remember(base: &str, policy: Policy) {
    if let Ok(mut guard) = POLICY.lock() {
        match guard.iter_mut().find(|(b, _)| b == base) {
            Some(slot) => slot.1 = policy,
            None => guard.push((base.to_string(), policy)),
        }
    }
}

/// `"https://api.openai.com"` → `…/v1`. A bare origin gets the version
/// segment, because that is where the OpenAI routes live (and where Ollama
/// puts its OpenAI-compatible API too); anything that already has a path is
/// taken at face value.
fn normalise_base(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return DEFAULT_BASE_URL.to_string();
    }
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    if !without_scheme.contains('/') {
        return format!("{trimmed}/v1");
    }
    trimmed.to_string()
}

fn build(base: &str, policy: &Policy, model: &str, history: &[Value]) -> (String, Value) {
    match policy.endpoint {
        Endpoint::Responses => {
            let mut body = json!({
                "model": model,
                "instructions": SYSTEM_PROMPT,
                "input": render_responses(history, policy),
                "max_output_tokens": MAX_TOKENS,
            });
            if policy.search {
                body["tools"] = json!([{ "type": "web_search" }]);
            }
            (format!("{base}/responses"), body)
        }
        Endpoint::Chat => (
            format!("{base}/chat/completions"),
            json!({
                "model": model,
                "messages": render_chat(history, policy),
                // `max_tokens`, not `max_completion_tokens`: this path exists
                // precisely for servers that clone the original API, and every
                // one of them accepts this name.
                "max_tokens": MAX_TOKENS,
            }),
        ),
    }
}

// ── History rendering ─────────────────────────────────────────────────────────
//
// `chat.rs` stores turns in a neutral shape — `{ role, parts[] }`, parts being
// text / image / document — so that a conversation can continue across a
// route change. Each route renders it into its own JSON.

fn text_part(text: &str) -> Value {
    json!({ "kind": "text", "text": text })
}

fn media_part(kind: &str, name: &str, media_type: &str, data: &str) -> Value {
    json!({ "kind": kind, "name": name, "mediaType": media_type, "data": data })
}

fn part_text(part: &Value) -> &str {
    part.get("text").and_then(Value::as_str).unwrap_or("")
}

fn data_uri(part: &Value) -> String {
    let media = part.get("mediaType").and_then(Value::as_str).unwrap_or("application/octet-stream");
    let data = part.get("data").and_then(Value::as_str).unwrap_or("");
    format!("data:{media};base64,{data}")
}

fn context_parts(context: &Option<ChatContext>) -> Option<Vec<Value>> {
    match context {
        Some(ChatContext::File { name, path }) => {
            let mut parts = Vec::new();
            if let Some(content) = chat::read_file(path) {
                parts.push(match content {
                    FileContent::Text(text) => text_part(&format!("File contents:\n{text}")),
                    FileContent::Image { media_type, data } => media_part(
                        "image",
                        name,
                        media_type,
                        &chat::base64_for(&data),
                    ),
                    FileContent::Document { media_type, data } => media_part(
                        "document",
                        name,
                        media_type,
                        &chat::base64_for(&data),
                    ),
                });
            }
            parts.push(text_part(&format!("File: {name}")));
            Some(parts)
        }
        Some(ChatContext::Window { app_name, title, url }) => {
            let mut text = format!("Context — App: {app_name}, Window: {title}");
            if let Some(url) = url {
                text.push_str(&format!(", URL: {url}"));
            }
            Some(vec![text_part(&text)])
        }
        None => None,
    }
}

/// `type` is not optional in practice. OpenAI's "easy input" lets you omit it,
/// but llama.cpp's Responses parser refuses an assistant item without it —
/// 400 `Cannot determine type of 'item'`, i.e. every turn after the first.
/// Sending the documented form costs nothing and works on both.
fn render_responses(history: &[Value], policy: &Policy) -> Vec<Value> {
    history
        .iter()
        .filter_map(|message| {
            let role = message.get("role").and_then(Value::as_str)?;
            let parts = message.get("parts").and_then(Value::as_array)?;
            let content: Vec<Value> = parts
                .iter()
                .filter_map(|p| {
                    let kind = p.get("kind").and_then(Value::as_str)?;
                    if !policy.media && (kind == "image" || kind == "document") {
                        return Some(json!({ "type": "input_text", "text": media_note(kind, p) }));
                    }
                    match kind {
                        "text" => Some(json!({ "type": "input_text", "text": part_text(p) })),
                        "image" => Some(json!({ "type": "input_image", "image_url": data_uri(p) })),
                        "document" => Some(json!({
                            "type": "input_file",
                            "file_url": data_uri(p),
                            "filename": p.get("name").and_then(Value::as_str).unwrap_or("attachment"),
                        })),
                        _ => None,
                    }
                })
                .collect();
            (!content.is_empty()).then(|| {
                let role = if role == "assistant" { "assistant" } else { "user" };
                let mut item = json!({ "type": "message", "role": role, "content": content });
                if role == "assistant" {
                    // Required by the spec, tolerated as absent, but the strict
                    // servers are the ones that needed fixing anyway.
                    item["status"] = json!("completed");
                }
                item
            })
        })
        .collect()
}

/// What to tell the model when the server cannot take the attachment at all.
/// The turn still goes through, minus the bytes, rather than erroring out.
fn media_note(kind: &str, part: &Value) -> String {
    let name = part.get("name").and_then(Value::as_str).unwrap_or("attachment");
    let what = if kind == "image" { "images" } else { "PDFs" };
    format!("[{name} was attached, but this endpoint cannot read {what}]")
}

fn has_media(history: &[Value]) -> bool {
    history.iter().any(|message| {
        message
            .get("parts")
            .and_then(Value::as_array)
            .map_or(false, |parts| {
                parts.iter().any(|p| {
                    matches!(p.get("kind").and_then(Value::as_str), Some("image" | "document"))
                })
            })
    })
}

fn render_chat(history: &[Value], policy: &Policy) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];

    for message in history {
        let Some(role) = message.get("role").and_then(Value::as_str) else { continue };
        let Some(parts) = message.get("parts").and_then(Value::as_array) else { continue };

        let mut texts: Vec<String> = Vec::new();
        let mut items: Vec<Value> = Vec::new();
        let mut carries_media = false;

        for part in parts {
            match part.get("kind").and_then(Value::as_str) {
                Some("text") => {
                    let text = part_text(part).to_string();
                    items.push(json!({ "type": "text", "text": text }));
                    texts.push(text);
                }
                // Chat Completions takes images but has no file part, and a
                // server that refused attachments gets them as a note instead.
                Some(kind) if kind == "image" && policy.media => {
                    items.push(json!({ "type": "image_url", "image_url": { "url": data_uri(part) } }));
                    carries_media = true;
                }
                Some(kind) if kind == "image" || kind == "document" => {
                    let note = media_note(kind, part);
                    items.push(json!({ "type": "text", "text": note }));
                    texts.push(note);
                }
                _ => {}
            }
        }
        if texts.is_empty() && !carries_media {
            continue;
        }

        out.push(json!({
            "role": if role == "assistant" { "assistant" } else { "user" },
            "content": if carries_media { Value::Array(items) } else { Value::String(texts.join("\n\n")) },
        }));
    }
    out
}

// ── Response parsing ──────────────────────────────────────────────────────────

fn extract(policy: &Policy, body: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| format!("Bad API response: {e}"))?;

    match policy.endpoint {
        Endpoint::Responses => {
            let mut texts: Vec<&str> = Vec::new();
            if let Some(output) = value.get("output").and_then(Value::as_array) {
                for item in output {
                    if item.get("type").and_then(Value::as_str) != Some("message") {
                        continue;
                    }
                    let Some(content) = item.get("content").and_then(Value::as_array) else {
                        continue;
                    };
                    for block in content {
                        match block.get("type").and_then(Value::as_str) {
                            Some("output_text") => {
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    texts.push(text);
                                }
                            }
                            Some("refusal") => {
                                let why = block
                                    .get("refusal")
                                    .and_then(Value::as_str)
                                    .unwrap_or("The model declined this one.");
                                return Err(why.to_string());
                            }
                            _ => {}
                        }
                    }
                }
            }
            let text = texts.join("\n").trim().to_string();
            if text.is_empty() {
                return Err(incomplete_reason(&value)
                    .unwrap_or_else(|| "No response text.".to_string()));
            }
            Ok(text)
        }
        Endpoint::Chat => {
            let message = value.pointer("/choices/0/message");
            if let Some(why) = message
                .and_then(|m| m.get("refusal"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                return Err(why.to_string());
            }
            if value.pointer("/choices/0/finish_reason").and_then(Value::as_str)
                == Some("content_filter")
            {
                return Err("The model declined this one.".into());
            }
            let text = message
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if text.is_empty() {
                return Err("No response text.".into());
            }
            Ok(text)
        }
    }
}

/// Why a Responses call came back without text. `incomplete_details.reason` is
/// "refusal" for a policy decline — the equivalent of Anthropic's
/// `stop_reason: "refusal"`.
fn incomplete_reason(value: &Value) -> Option<String> {
    let reason = value.pointer("/incomplete_details/reason").and_then(Value::as_str)?;
    Some(match reason {
        "refusal" => "The model declined this one.".into(),
        "max_output_tokens" => "The reply ran into the output limit.".into(),
        other => format!("Incomplete response: {other}"),
    })
}

fn mentions(haystack: &str, needles: &[&str]) -> bool {
    let haystack = haystack.to_lowercase();
    needles.iter().any(|needle| haystack.contains(&needle.to_lowercase()))
}

struct Response {
    status: reqwest::StatusCode,
    body: String,
}

async fn post(url: &str, key: Option<&str>, body: &Value) -> Result<Response, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;

    // No header at all when no key is stored: a local server is happy either
    // way, and it avoids sending an empty bearer token it might reject.
    let mut request = client.post(url).header("content-type", "application/json");
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }

    let response = request
        .json(body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    Ok(Response { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: &str, parts: Vec<Value>) -> Value {
        json!({ "role": role, "parts": parts })
    }

    /// A policy that allows everything — what a well-behaved server gets.
    fn allowing() -> Policy {
        Policy { endpoint: Endpoint::Responses, search: true, media: true }
    }

    #[test]
    fn a_bare_origin_gains_the_v1_segment() {
        assert_eq!(normalise_base("https://api.openai.com"), "https://api.openai.com/v1");
        assert_eq!(normalise_base("http://localhost:11434"), "http://localhost:11434/v1");
        assert_eq!(normalise_base("http://localhost:1234/v1/"), "http://localhost:1234/v1");
        assert_eq!(normalise_base("  "), DEFAULT_BASE_URL);
    }

    #[test]
    fn chat_completions_keeps_text_only_turns_as_plain_strings() {
        let history = vec![
            turn("user", vec![text_part("hello"), text_part("second line")]),
            turn("assistant", vec![text_part("hi")]),
        ];
        let rendered = render_chat(&history, &allowing());

        assert_eq!(rendered[0]["role"], "system");
        assert_eq!(rendered[1]["content"], "hello\n\nsecond line");
        assert_eq!(rendered[2]["role"], "assistant");
        assert_eq!(rendered[2]["content"], "hi");
    }

    #[test]
    fn a_pdf_still_reaches_the_model_on_chat_completions() {
        let history = vec![turn(
            "user",
            vec![
                media_part("document", "notes.pdf", "application/pdf", "JVBERi0="),
                text_part("File: notes.pdf"),
            ],
        )];
        let content = render_chat(&history, &allowing())[1]["content"].clone();
        let text = content.as_str().expect("text-only turn stays a string");
        assert!(text.contains("notes.pdf was attached"));
    }

    #[test]
    fn the_responses_route_maps_parts_to_input_blocks() {
        let history = vec![turn("user", vec![text_part("hello")])];
        let rendered = render_responses(&history, &allowing());

        assert_eq!(rendered[0]["role"], "user");
        assert_eq!(rendered[0]["content"][0]["type"], "input_text");
        assert_eq!(rendered[0]["content"][0]["text"], "hello");
    }

    #[test]
    fn every_responses_item_carries_the_type_llamacpp_needs() {
        // Seen live: llama.cpp answers 400 `Cannot determine type of 'item'`
        // to an item that has role+content but no type — which, without this,
        // is every turn after the first. OpenAI accepts the same body either way.
        let history = vec![
            turn("user", vec![text_part("hi")]),
            turn("assistant", vec![text_part("hello there")]),
        ];
        let rendered = render_responses(&history, &allowing());

        assert_eq!(rendered[0]["type"], "message");
        assert_eq!(rendered[1]["type"], "message");
        assert_eq!(rendered[1]["role"], "assistant");
        assert_eq!(rendered[1]["status"], "completed");
    }

    #[test]
    fn an_attachment_the_server_refuses_becomes_words_not_an_error() {
        let history = vec![turn(
            "user",
            vec![
                media_part("image", "shot.png", "image/png", "AAAA"),
                text_part("File: shot.png"),
                text_part("what is this"),
            ],
        )];
        assert!(has_media(&history));
        let mute = Policy { endpoint: Endpoint::Responses, search: true, media: false };

        let responses = render_responses(&history, &mute);
        assert_eq!(responses[0]["content"][0]["type"], "input_text");
        assert!(responses[0]["content"][0]["text"].as_str().unwrap().contains("shot.png"));
        // no stray base64 anywhere in the muted body
        assert!(!responses[0]["content"].to_string().contains("AAAA"));

        let chat = render_chat(&history, &mute);
        let content = chat[1]["content"].as_str().expect("no media left, so a plain string");
        assert!(content.contains("shot.png was attached"));
        assert!(content.contains("what is this"));
    }

    #[test]
    fn responses_text_is_joined_across_output_messages() {
        let body = json!({
            "output": [
                { "type": "message", "content": [{ "type": "output_text", "text": "one" }] },
                { "type": "web_search_call" },
                { "type": "message", "content": [{ "type": "output_text", "text": "two" }] },
            ]
        });
        let policy = allowing();
        assert_eq!(extract(&policy, &body.to_string()).unwrap(), "one\ntwo");
    }

    #[test]
    fn a_responses_refusal_becomes_an_error() {
        let body = json!({
            "output": [{ "type": "message", "content": [{ "type": "refusal", "refusal": "Not doing that." }] }]
        });
        let policy = allowing();
        assert_eq!(extract(&policy, &body.to_string()).unwrap_err(), "Not doing that.");
    }

    #[test]
    fn chat_completions_reads_the_first_choice() {
        let body = json!({ "choices": [{ "message": { "content": "  hi  " }, "finish_reason": "stop" }] });
        let policy = Policy { endpoint: Endpoint::Chat, search: false, media: true };
        assert_eq!(extract(&policy, &body.to_string()).unwrap(), "hi");
    }

    // ── End to end, against a local server ────────────────────────────────────

    const RESPONSES_OK: &str = r#"{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"mock replies"}]}]}"#;
    const CHAT_OK: &str =
        r#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"mock replies"}}]}"#;
    const NOT_FOUND: &str = r#"{"error":{"message":"Unknown URL."}}"#;

    /// Speaks just enough HTTP to prove the routing: 404 on any route outside
    /// `routes`, a canned body on the ones inside, every request recorded.
    /// No network, no key — which is the point of testing the fallback at all.
    fn mock_server(routes: &[&'static str]) -> (String, std::sync::Arc<Mutex<Vec<Value>>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::Arc;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes = routes.to_vec();
        let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());

                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("").to_string();

                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }

                let mut bytes = vec![0u8; length];
                if length > 0 && reader.read_exact(&mut bytes).is_err() {
                    continue;
                }
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                record.lock().unwrap().push(json!({ "path": path, "body": body }));

                let (status, payload) = if routes.iter().any(|route| path.ends_with(*route)) {
                    ("200 OK", if path.ends_with("/responses") { RESPONSES_OK } else { CHAT_OK })
                } else {
                    ("404 Not Found", NOT_FOUND)
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });

        (base, seen)
    }

    /// tokio is a dependency of the app but without `macros`, so the test
    /// builds its own current-thread runtime rather than growing the manifest.
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    }

    #[test]
    fn a_server_without_responses_falls_back_to_chat_completions() {
        let (base, seen) = mock_server(&["/chat/completions"]);
        let chat = Chat::default();

        let reply = runtime()
            .block_on(send(&chat, &base, "gpt-test", "hello".to_string(), None))
            .expect("the fallback turn succeeds");

        assert_eq!(reply.text, "mock replies");

        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 2, "one probe, one real turn");
        assert!(requests[0]["path"].as_str().unwrap().ends_with("/responses"));
        assert_eq!(requests[1]["path"], "/v1/chat/completions");
        assert_eq!(requests[1]["body"]["model"], "gpt-test");
        assert_eq!(requests[1]["body"]["messages"][0]["role"], "system");
        drop(requests);

        // The user turn and the assistant turn, both kept for the next one.
        assert_eq!(chat.snapshot().len(), 2);
    }

    #[test]
    fn the_route_that_worked_is_reused_by_the_next_turn() {
        let (base, seen) = mock_server(&["/chat/completions"]);
        let chat = Chat::default();
        let rt = runtime();

        rt.block_on(send(&chat, &base, "gpt-test", "one".to_string(), None)).unwrap();
        rt.block_on(send(&chat, &base, "gpt-test", "two".to_string(), None)).unwrap();

        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3, "the 404 is paid once, not once per turn");
        assert_eq!(requests[2]["path"], "/v1/chat/completions");
        // system, first user turn, assistant, second user turn
        assert_eq!(requests[2]["body"]["messages"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn a_working_responses_route_is_used_as_is() {
        let (base, seen) = mock_server(&["/responses", "/chat/completions"]);
        let chat = Chat::default();

        let reply = runtime()
            .block_on(send(&chat, &base, "gpt-test", "hello".to_string(), None))
            .unwrap();
        assert_eq!(reply.text, "mock replies");

        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1, "no probe was needed");
        assert_eq!(requests[0]["path"], "/v1/responses");
        assert!(requests[0]["body"]["instructions"].as_str().is_some());
        assert_eq!(requests[0]["body"]["tools"][0]["type"], "web_search");
    }
}
