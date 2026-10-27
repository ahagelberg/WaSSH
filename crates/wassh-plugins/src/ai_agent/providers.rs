//! Provider calls: request bodies, streaming parsers and the HTTP client.
//!
//! Mirrors `src/plugins/builtins/ai-agent/providers.ts`. The parsers are pure so
//! they can be tested against captured payloads; the call itself runs on its own
//! thread and reports [`StreamEvent`]s over a channel, so the pane never blocks.

use std::sync::mpsc::Sender;

use serde_json::{json, Value};

use super::protocol::{
    join_url, ApiMessage, ApiToolCall, Completion, Protocol, ProviderConfig, ANTHROPIC_CHAT_PATH,
    ANTHROPIC_MODELS_PATH, ANTHROPIC_VERSION, OPENAI_CHAT_PATH, OPENAI_EMBEDDINGS_PATH,
    OPENAI_MODELS_PATH,
};

/// Cap on the error text attached to a failed call.
pub const MAX_ERROR_BODY_CHARS: usize = 500;

/// Said when the endpoint answered but streamed no events at all.
pub const NO_STREAM: &str = "The provider sent no event stream — check the base URL and the key.";

/// One completion to request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionRequest {
    /// Endpoint, without the operation path.
    pub base_url: String,
    /// API key; empty for local servers.
    pub api_key: String,
    /// Wire format.
    pub protocol: Protocol,
    /// Model name.
    pub model: String,
    /// System prompt.
    pub system: String,
    /// Conversation so far.
    pub messages: Vec<ApiMessage>,
    /// Tools the model may call, as JSON schemas.
    pub tools: Vec<Value>,
    /// Token budget (Anthropic needs one; OpenAI picks its own default).
    pub max_tokens: u32,
}

/// What a running call reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    /// A piece of assistant text.
    Delta(String),
    /// The call finished.
    Done(Completion),
    /// The call failed.
    Failed(String),
}

/// The request body for an OpenAI-compatible endpoint.
pub fn openai_body(request: &CompletionRequest) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": request.system })];
    for message in &request.messages {
        messages.push(match message {
            ApiMessage::User { content } => json!({ "role": "user", "content": content }),
            ApiMessage::Tool {
                tool_call_id,
                content,
            } => json!({
                "role": "tool",
                "tool_call_id": tool_call_id,
                "content": content
            }),
            ApiMessage::Assistant {
                content,
                tool_calls,
            } => json!({
                "role": "assistant",
                "content": content,
                "tool_calls": tool_calls
                    .iter()
                    .map(|call| json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.name, "arguments": call.arguments }
                    }))
                    .collect::<Vec<_>>()
            }),
        });
    }
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true
    });
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool["name"],
                            "description": tool["description"],
                            "parameters": tool["parameters"]
                        }
                    })
                })
                .collect(),
        );
    }
    body
}

/// Parse the tool-call arguments, empty object when they are not JSON.
fn parse_arguments(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!({}))
}

/// The request body for the Anthropic messages API.
///
/// Anthropic needs strictly alternating roles, so consecutive user turns are
/// folded together and tool results ride inside the following user turn.
pub fn anthropic_body(request: &CompletionRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message {
            ApiMessage::User { content } => {
                if let Some(last) = messages.last_mut()
                    && last["role"] == "user"
                {
                    let previous = last["content"].as_array().cloned().unwrap_or_else(|| {
                        vec![json!({ "type": "text", "text": last["content"].clone() })]
                    });
                    let mut blocks = previous;
                    blocks.push(json!({ "type": "text", "text": content }));
                    last["content"] = Value::Array(blocks);
                } else {
                    messages.push(json!({ "role": "user", "content": content }));
                }
            }
            ApiMessage::Tool {
                tool_call_id,
                content,
            } => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content
                });
                if let Some(last) = messages.last_mut()
                    && last["role"] == "user"
                    && last["content"].is_array()
                {
                    last["content"]
                        .as_array_mut()
                        .expect("checked above")
                        .push(block);
                } else {
                    messages.push(json!({ "role": "user", "content": [block] }));
                }
            }
            ApiMessage::Assistant {
                content,
                tool_calls,
            } => {
                let mut blocks = Vec::new();
                if !content.is_empty() {
                    blocks.push(json!({ "type": "text", "text": content }));
                }
                for call in tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": parse_arguments(&call.arguments)
                    }));
                }
                messages.push(json!({ "role": "assistant", "content": blocks }));
            }
        }
    }

    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "system": request.system,
        "messages": messages,
        "stream": true
    });
    if !request.tools.is_empty() {
        body["tools"] = Value::Array(
            request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool["name"],
                        "description": tool["description"],
                        "input_schema": tool["parameters"]
                    })
                })
                .collect(),
        );
    }
    body
}

/// Header pairs for one call.
pub fn headers(request: &CompletionRequest) -> Vec<(String, String)> {
    let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
    match request.protocol {
        Protocol::Openai => {
            if !request.api_key.is_empty() {
                headers.push(("authorization".to_owned(), format!("Bearer {}", request.api_key)));
            }
        }
        Protocol::Anthropic => {
            headers.push(("x-api-key".to_owned(), request.api_key.clone()));
            headers.push(("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned()));
        }
    }
    headers
}

/// The chat path for a protocol.
pub fn chat_path(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Openai => OPENAI_CHAT_PATH,
        Protocol::Anthropic => ANTHROPIC_CHAT_PATH,
    }
}

/// The model-list path for a protocol.
pub fn models_path(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Openai => OPENAI_MODELS_PATH,
        Protocol::Anthropic => ANTHROPIC_MODELS_PATH,
    }
}

/// Accumulates an OpenAI stream.
#[derive(Default)]
pub struct OpenAiStream {
    text: String,
    fragments: std::collections::BTreeMap<u64, (String, String, String)>,
    stop_reason: Option<String>,
}

impl OpenAiStream {
    /// Feed one `data:` payload and return the text it added, if any.
    pub fn push(&mut self, payload: &str) -> Option<String> {
        let json: Value = serde_json::from_str(payload).ok()?;
        if let Some(reason) = json["choices"][0]["finish_reason"].as_str()
            && !reason.is_empty()
        {
            self.stop_reason = Some(reason.to_owned());
        }
        let delta = &json["choices"][0]["delta"];
        let mut added = None;
        if let Some(content) = delta["content"].as_str()
            && !content.is_empty()
        {
            self.text.push_str(content);
            added = Some(content.to_owned());
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for call in calls {
                let index = call["index"].as_u64().unwrap_or(0);
                let fragment = self.fragments.entry(index).or_default();
                if let Some(id) = call["id"].as_str()
                    && !id.is_empty()
                {
                    fragment.0 = id.to_owned();
                }
                if let Some(name) = call["function"]["name"].as_str()
                    && !name.is_empty()
                {
                    fragment.1.push_str(name);
                }
                if let Some(arguments) = call["function"]["arguments"].as_str() {
                    fragment.2.push_str(arguments);
                }
            }
        }
        added
    }

    /// The finished completion.
    pub fn finish(self) -> Completion {
        let tool_calls = self
            .fragments
            .into_iter()
            .filter(|(_, (_, name, _))| !name.is_empty())
            .map(|(index, (id, name, arguments))| ApiToolCall {
                id: if id.is_empty() {
                    format!("call_{index}")
                } else {
                    id
                },
                name,
                arguments,
            })
            .collect();
        Completion {
            text: self.text,
            tool_calls,
            stop_reason: self.stop_reason,
        }
    }
}

/// Accumulates an Anthropic stream.
#[derive(Default)]
pub struct AnthropicStream {
    text: String,
    blocks: std::collections::BTreeMap<u64, (String, String, String)>,
    stop_reason: Option<String>,
}

impl AnthropicStream {
    /// Feed one `data:` payload and return the text it added, if any.
    pub fn push(&mut self, payload: &str) -> Option<String> {
        let json: Value = serde_json::from_str(payload).ok()?;
        match json["type"].as_str().unwrap_or_default() {
            "content_block_start" => {
                let block = &json["content_block"];
                if block["type"] == "tool_use" {
                    let index = json["index"].as_u64().unwrap_or(0);
                    let id = block["id"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("call_{index}"));
                    let name = block["name"].as_str().unwrap_or_default().to_owned();
                    self.blocks.insert(index, (id, name, String::new()));
                }
                None
            }
            "content_block_delta" => {
                let delta = &json["delta"];
                if delta["type"] == "text_delta" {
                    let text = delta["text"].as_str().unwrap_or_default();
                    if text.is_empty() {
                        return None;
                    }
                    self.text.push_str(text);
                    return Some(text.to_owned());
                }
                if delta["type"] == "input_json_delta" {
                    let index = json["index"].as_u64().unwrap_or(0);
                    if let Some(block) = self.blocks.get_mut(&index)
                        && let Some(partial) = delta["partial_json"].as_str()
                    {
                        block.2.push_str(partial);
                    }
                }
                None
            }
            "message_delta" => {
                if let Some(reason) = json["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_owned());
                }
                None
            }
            _ => None,
        }
    }

    /// The finished completion.
    pub fn finish(self) -> Completion {
        Completion {
            text: self.text,
            tool_calls: self
                .blocks
                .into_iter()
                .filter(|(_, (_, name, _))| !name.is_empty())
                .map(|(_, (id, name, arguments))| ApiToolCall {
                    id,
                    name,
                    arguments,
                })
                .collect(),
            stop_reason: self.stop_reason,
        }
    }
}

/// Whether a line carries an SSE payload, and what it is.
pub fn sse_payload(line: &str) -> Option<&str> {
    let payload = line.trim_end_matches('\r').strip_prefix("data:")?.trim();
    if payload.is_empty() || payload == "[DONE]" {
        None
    } else {
        Some(payload)
    }
}

/// Run one completion, reporting events to `events`.
///
/// Blocking: call it from a worker thread.
pub fn complete(request: &CompletionRequest, events: &Sender<StreamEvent>) {
    let body = match request.protocol {
        Protocol::Openai => openai_body(request),
        Protocol::Anthropic => anthropic_body(request),
    };
    let url = join_url(&request.base_url, chat_path(request.protocol));
    let mut http = ureq::post(&url);
    for (name, value) in headers(request) {
        http = http.header(&name, &value);
    }

    let response = match http.send_json(&body) {
        Ok(response) => response,
        Err(error) => {
            let _ = events.send(StreamEvent::Failed(describe_error(error)));
            return;
        }
    };

    let mut reader = response.into_body().into_reader();
    let mut openai = OpenAiStream::default();
    let mut anthropic = AnthropicStream::default();
    let mut line = String::new();
    let mut saw_payload = false;
    loop {
        line.clear();
        // Read one line at a time so deltas reach the pane as they arrive.
        match read_line(&mut reader, &mut line) {
            Ok(true) => {}
            Ok(false) => break,
            Err(error) => {
                let _ = events
                    .send(StreamEvent::Failed(shorten(format!("reading the stream: {error}"))));
                return;
            }
        }
        let Some(payload) = sse_payload(&line) else {
            continue;
        };
        saw_payload = true;
        let added = match request.protocol {
            Protocol::Openai => openai.push(payload),
            Protocol::Anthropic => anthropic.push(payload),
        };
        if let Some(text) = added
            && events.send(StreamEvent::Delta(text)).is_err()
        {
            return;
        }
    }

    if !saw_payload {
        // A body with no events at all is an error page or the wrong endpoint, not an answer.
        let _ = events.send(StreamEvent::Failed(NO_STREAM.to_owned()));
        return;
    }
    let completion = match request.protocol {
        Protocol::Openai => openai.finish(),
        Protocol::Anthropic => anthropic.finish(),
    };
    let _ = events.send(StreamEvent::Done(completion));
}

/// Read one line into `out`, newline dropped; false at the end of the stream.
///
/// A blank line is a line of its own — SSE separates events with one — so only
/// end of stream ends the read.
fn read_line(reader: &mut impl std::io::Read, out: &mut String) -> std::io::Result<bool> {
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte)? {
            0 => {
                out.push_str(&String::from_utf8_lossy(&bytes));
                return Ok(!bytes.is_empty());
            }
            _ if byte[0] == b'\n' => {
                out.push_str(&String::from_utf8_lossy(&bytes));
                return Ok(true);
            }
            _ => bytes.push(byte[0]),
        }
    }
}

/// Cut a message down to the length the reference shows.
fn shorten(text: String) -> String {
    if text.chars().count() <= MAX_ERROR_BODY_CHARS {
        return text;
    }
    let mut short: String = text.chars().take(MAX_ERROR_BODY_CHARS).collect();
    short.push('…');
    short
}

/// A readable description of a failed request.
fn describe_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(code) => format!("Provider error {code}"),
        other => shorten(format!("Provider request failed: {other}")),
    }
}

/// Ask a provider for its model list.
pub fn provider_models(provider: &ProviderConfig, api_key: &str) -> Result<Vec<String>, String> {
    let url = join_url(&provider.base_url, models_path(provider.protocol));
    let mut http = ureq::get(&url);
    match provider.protocol {
        Protocol::Openai => {
            if !api_key.is_empty() {
                http = http.header("authorization", &format!("Bearer {api_key}"));
            }
        }
        Protocol::Anthropic => {
            http = http.header("x-api-key", api_key);
            http = http.header("anthropic-version", ANTHROPIC_VERSION);
        }
    }
    let response = http.call().map_err(|error| match error {
        ureq::Error::StatusCode(code) => format!("Provider error {code}"),
        other => format!("Provider request failed: {other}"),
    })?;
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("reading the model list: {error}"))?;
    let json: Value =
        serde_json::from_str(&text).map_err(|error| format!("unexpected model list: {error}"))?;
    let mut models = Vec::new();
    if let Some(items) = json["data"].as_array() {
        for item in items {
            if let Some(id) = item["id"].as_str() {
                models.push(id.to_owned());
            }
        }
    } else if let Some(items) = json["models"].as_array() {
        for item in items {
            if let Some(id) = item["id"].as_str() {
                models.push(id.to_owned());
            }
        }
    }
    models.sort();
    models.dedup();
    Ok(models)
}

/// Settings for the embedding model behind `rag_search`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedConfig {
    /// Provider endpoint, without the operation path.
    pub base_url: String,
    /// Provider key.
    pub api_key: String,
    /// Wire format.
    pub protocol: Protocol,
    /// Embedding model name.
    pub model: String,
}

/// One non-streaming completion, for summaries.
///
/// Blocking: call it from a worker thread.
pub fn complete_blocking(request: &CompletionRequest) -> Result<Completion, String> {
    let mut body = match request.protocol {
        Protocol::Openai => openai_body(request),
        Protocol::Anthropic => anthropic_body(request),
    };
    body["stream"] = Value::Bool(false);
    let url = join_url(&request.base_url, chat_path(request.protocol));
    let mut http = ureq::post(&url);
    for (name, value) in headers(request) {
        http = http.header(&name, &value);
    }
    let response = http.send_json(&body).map_err(describe_error)?;
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("reading the reply: {error}"))?;
    let json: Value =
        serde_json::from_str(&text).map_err(|error| shorten(format!("unexpected reply: {error}")))?;
    match request.protocol {
        Protocol::Openai => {
            let message = &json["choices"][0]["message"];
            let tool_calls = message["tool_calls"]
                .as_array()
                .map(|calls| {
                    calls
                        .iter()
                        .filter_map(|call| {
                            Some(ApiToolCall {
                                id: call["id"].as_str()?.to_owned(),
                                name: call["function"]["name"].as_str()?.to_owned(),
                                arguments: call["function"]["arguments"].as_str().unwrap_or("{}").to_owned(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(Completion {
                text: message["content"].as_str().unwrap_or_default().to_owned(),
                tool_calls,
                stop_reason: json["choices"][0]["finish_reason"].as_str().map(str::to_owned),
            })
        }
        Protocol::Anthropic => {
            let mut text = String::new();
            let mut tool_calls = Vec::new();
            if let Some(blocks) = json["content"].as_array() {
                for block in blocks {
                    match block["type"].as_str().unwrap_or_default() {
                        "text" => text.push_str(block["text"].as_str().unwrap_or_default()),
                        "tool_use" => tool_calls.push(ApiToolCall {
                            id: block["id"].as_str().unwrap_or_default().to_owned(),
                            name: block["name"].as_str().unwrap_or_default().to_owned(),
                            arguments: block["input"].to_string(),
                        }),
                        _ => {}
                    }
                }
            }
            Ok(Completion {
                text,
                tool_calls,
                stop_reason: json["stop_reason"].as_str().map(str::to_owned),
            })
        }
    }
}

/// Embed texts with an OpenAI-compatible provider.
pub fn embeddings(config: &EmbedConfig, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if config.model.trim().is_empty() {
        return Err("No embedding model is configured.".to_owned());
    }
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let url = join_url(&config.base_url, OPENAI_EMBEDDINGS_PATH);
    let body = serde_json::json!({ "model": config.model.trim(), "input": texts });
    let mut http = ureq::post(&url);
    if config.protocol == Protocol::Openai && !config.api_key.is_empty() {
        http = http.header("authorization", &format!("Bearer {}", config.api_key));
    }
    let response = http.send_json(&body).map_err(describe_error)?;
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("reading the embeddings: {error}"))?;
    let json: Value = serde_json::from_str(&text)
        .map_err(|error| shorten(format!("unexpected embedding reply: {error}")))?;
    let mut vectors = Vec::new();
    if let Some(items) = json["data"].as_array() {
        for item in items {
            let vector: Vec<f32> = item["embedding"]
                .as_array()
                .map(|values| values.iter().filter_map(Value::as_f64).map(|value| value as f32).collect())
                .unwrap_or_default();
            vectors.push(vector);
        }
    }
    if vectors.len() != texts.len() {
        return Err(format!(
            "The embedding provider returned {} vectors for {} passages.",
            vectors.len(),
            texts.len()
        ));
    }
    Ok(vectors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(protocol: Protocol) -> CompletionRequest {
        CompletionRequest {
            base_url: "https://example.test/v1".to_owned(),
            api_key: "secret".to_owned(),
            protocol,
            model: "m".to_owned(),
            system: "be brief".to_owned(),
            messages: vec![
                ApiMessage::User {
                    content: "hi".to_owned(),
                },
                ApiMessage::Assistant {
                    content: "hello".to_owned(),
                    tool_calls: vec![ApiToolCall {
                        id: "c1".to_owned(),
                        name: "run_command".to_owned(),
                        arguments: r#"{"command":"uptime"}"#.to_owned(),
                    }],
                },
                ApiMessage::Tool {
                    tool_call_id: "c1".to_owned(),
                    content: "up 3 days".to_owned(),
                },
            ],
            tools: vec![json!({
                "name": "run_command",
                "description": "run it",
                "parameters": { "type": "object" }
            })],
            max_tokens: 1024,
        }
    }

    #[test]
    fn the_openai_body_uses_function_tools() {
        let body = openai_body(&request(Protocol::Openai));
        assert_eq!(body["model"], "m");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
        assert_eq!(body["messages"][2]["tool_calls"][0]["function"]["name"], "run_command");
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["messages"][3]["tool_call_id"], "c1");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn the_anthropic_body_folds_tool_results_into_user_turns() {
        let body = anthropic_body(&request(Protocol::Anthropic));
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["max_tokens"], 1024);
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "hi");
        // The assistant turn carries text plus the tool_use block.
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["text"], "hello");
        assert_eq!(messages[1]["content"][1]["type"], "tool_use");
        assert_eq!(messages[1]["content"][1]["input"]["command"], "uptime");
        // The tool result rides in a user turn of its own.
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "c1");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn consecutive_user_turns_are_folded_for_anthropic() {
        let mut req = request(Protocol::Anthropic);
        req.messages = vec![
            ApiMessage::User {
                content: "one".to_owned(),
            },
            ApiMessage::User {
                content: "two".to_owned(),
            },
        ];
        let body = anthropic_body(&req);
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1, "Anthropic rejects repeated roles");
        assert_eq!(messages[0]["content"][1]["text"], "two");
    }

    #[test]
    fn headers_follow_the_protocol() {
        let openai = headers(&request(Protocol::Openai));
        assert!(openai.contains(&("authorization".to_owned(), "Bearer secret".to_owned())));
        let anthropic = headers(&request(Protocol::Anthropic));
        assert!(anthropic.contains(&("x-api-key".to_owned(), "secret".to_owned())));
        assert!(anthropic.contains(&("anthropic-version".to_owned(), ANTHROPIC_VERSION.to_owned())));
    }

    #[test]
    fn an_openai_stream_collects_text_and_tool_calls() {
        let mut stream = OpenAiStream::default();
        let payloads = [
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"run_","arguments":"{\"comm"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"command","arguments":"and\":\"uptime\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ];
        let mut text = String::new();
        for payload in payloads {
            if let Some(added) = stream.push(payload) {
                text.push_str(&added);
            }
        }
        let completion = stream.finish();
        assert_eq!(text, "Hello");
        assert_eq!(completion.text, "Hello");
        assert_eq!(completion.stop_reason.as_deref(), Some("tool_calls"));
        assert_eq!(completion.tool_calls.len(), 1);
        assert_eq!(completion.tool_calls[0].id, "c1");
        assert_eq!(completion.tool_calls[0].name, "run_command");
        assert_eq!(completion.tool_calls[0].arguments, r#"{"command":"uptime"}"#);
    }

    #[test]
    fn an_anthropic_stream_collects_text_and_tool_use() {
        let mut stream = AnthropicStream::default();
        let payloads = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"run_command"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"uptime\"}"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
        ];
        let mut text = String::new();
        for payload in payloads {
            if let Some(added) = stream.push(payload) {
                text.push_str(&added);
            }
        }
        let completion = stream.finish();
        assert_eq!(text, "Hi");
        assert_eq!(completion.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(completion.tool_calls.len(), 1);
        assert_eq!(completion.tool_calls[0].id, "toolu_1");
        assert_eq!(completion.tool_calls[0].name, "run_command");
        assert_eq!(completion.tool_calls[0].arguments, r#"{"command":"uptime"}"#);
    }

    #[test]
    fn sse_lines_are_recognised_and_done_is_skipped() {
        assert_eq!(sse_payload("data: {\"a\":1}\r"), Some("{\"a\":1}"));
        assert_eq!(sse_payload("event: ping"), None);
        assert_eq!(sse_payload("data: [DONE]"), None);
        assert_eq!(sse_payload("data:   "), None);
    }
}
