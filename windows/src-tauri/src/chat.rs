// Provider-neutral chat: the history, the turn the island sends, and the files
// it can attach. The wire formats live in `claude.rs` (Anthropic) and
// `openai.rs` (OpenAI and everything OpenAI-shaped).
//
// Nothing here knows about a particular API. That is the point: settings can
// switch provider between turns and the island, the IPC boundary and the file
// handling all stay where they are.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The assistant's system prompt — sent as `system` to Anthropic and as
/// `instructions` to the Responses API. The same words for every provider,
/// because "no markdown formatting" is what keeps the island's plain-text
/// renderer correct whichever model answers.
pub const SYSTEM_PROMPT: &str = "You are Mochi, a personal AI assistant living at the top of the user's screen. \
You have web search access and can help with absolutely anything — research, coding, finding places, recommendations, tasks, questions. \
Respond in the user's language. Be thorough and complete — use as much detail as the task requires. \
No markdown formatting (no **, no ##, no bullet dashes). Use plain text with line breaks.";

/// Text and code files are inlined; anything larger is skipped, as on macOS.
pub const MAX_INLINE_TEXT: u64 = 200_000;

#[derive(Default)]
struct ChatState {
    /// Which provider wrote `messages`. Empty until the first turn.
    provider: String,
    /// Full multi-turn history, in that provider's own shape.
    messages: Vec<Value>,
}

/// One conversation.
///
/// The stored JSON is whatever the *active* provider produced — Anthropic
/// block arrays for Claude (tool_use / tool_result included, so the next turn
/// keeps its web-search context), plain parts for OpenAI. Sending one to the
/// other would be meaningless, so `ensure_provider` starts a fresh
/// conversation when settings switch instead of guessing.
#[derive(Default)]
pub struct Chat {
    state: Mutex<ChatState>,
}

impl Chat {
    pub fn reset(&self) {
        self.state.lock().unwrap().messages.clear();
    }

    /// Drop the history if it was written by a different provider.
    pub fn ensure_provider(&self, provider: &str) {
        let mut state = self.state.lock().unwrap();
        if state.provider != provider {
            state.messages.clear();
            state.provider = provider.to_string();
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.state.lock().unwrap().messages.is_empty()
    }

    pub(crate) fn push(&self, message: Value) {
        self.state.lock().unwrap().messages.push(message);
    }

    pub(crate) fn pop(&self) {
        self.state.lock().unwrap().messages.pop();
    }

    pub(crate) fn snapshot(&self) -> Vec<Value> {
        self.state.lock().unwrap().messages.clone()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatContext {
    File { name: String, path: String },
    Window { app_name: String, title: String, url: Option<String> },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub text: String,
}

/// What a dropped file turned out to be, once read. Deliberately neutral:
/// Anthropic wraps it in a block array, OpenAI in content parts.
pub enum FileContent {
    /// Small enough to inline as text.
    Text(String),
    Image { media_type: &'static str, data: Vec<u8> },
    Document { media_type: &'static str, data: Vec<u8> },
}

/// PDF → document, image → image, text/code → inline text.
/// Mirrors readFileAsBlock() in ClaudeService.swift.
pub fn read_file(path: &str) -> Option<FileContent> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let media_type = match ext.as_str() {
        "pdf" => Some(("pdf", "application/pdf")),
        "jpg" | "jpeg" => Some(("image", "image/jpeg")),
        "png" => Some(("image", "image/png")),
        "gif" => Some(("image", "image/gif")),
        "webp" => Some(("image", "image/webp")),
        _ => None,
    };

    if let Some((kind, media)) = media_type {
        let data = std::fs::read(path).ok()?;
        return Some(if kind == "pdf" {
            FileContent::Document { media_type: media, data }
        } else {
            FileContent::Image { media_type: media, data }
        });
    }

    let len = std::fs::metadata(path).ok()?.len();
    if len > MAX_INLINE_TEXT {
        return None;
    }
    Some(FileContent::Text(std::fs::read_to_string(path).ok()?))
}

/// The API's own message out of an error body — what makes a bad key obvious.
/// Shared by both transports.
pub(crate) fn api_error(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(200).collect())
}

/// Small standalone base64 encoder — not worth another dependency.
/// Also used for Stripe's basic auth.
pub(crate) fn base64_for(bytes: &[u8]) -> String {
    base64(bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::base64;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_new_provider_starts_a_new_conversation() {
        let chat = super::Chat::default();
        chat.ensure_provider("claude");
        chat.push(json!({ "role": "user", "content": [] }));
        assert!(!chat.is_empty());

        // Switching drops Claude-shaped history rather than handing it to OpenAI.
        chat.ensure_provider("openai");
        assert!(chat.is_empty());
        assert_eq!(chat.snapshot().len(), 0);

        // Switching back to the same provider keeps the conversation.
        chat.push(json!({ "role": "user", "parts": [] }));
        chat.ensure_provider("openai");
        assert_eq!(chat.snapshot().len(), 1);
    }
}
