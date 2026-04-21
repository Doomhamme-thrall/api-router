use chrono::Utc;
use serde_json::{json, Value};

pub fn message_content_to_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(items) = content.as_array() {
        let mut texts = Vec::new();
        for item in items {
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                texts.push(text.to_string());
            }
        }
        return texts.join("\n");
    }
    String::new()
}

pub fn build_gemini_request_payload(openai_payload: &Value) -> Result<Value, String> {
    let Some(messages) = openai_payload.get("messages").and_then(|v| v.as_array()) else {
        return Err("gemini target requires openai messages[]".to_string());
    };

    let mut system_parts = Vec::new();
    let mut contents = Vec::new();
    for msg in messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let text = msg
            .get("content")
            .map(message_content_to_text)
            .unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }

        if role == "system" {
            system_parts.push(json!({"text": text}));
            continue;
        }

        let gemini_role = if role == "assistant" { "model" } else { "user" };
        contents.push(json!({
            "role": gemini_role,
            "parts": [{"text": text}]
        }));
    }

    if contents.is_empty() {
        return Err("gemini target requires at least one non-system message".to_string());
    }

    let mut out = json!({"contents": contents});

    let mut generation_config = serde_json::Map::new();
    if let Some(v) = openai_payload.get("temperature") {
        generation_config.insert("temperature".to_string(), v.clone());
    }
    if let Some(v) = openai_payload.get("top_p") {
        generation_config.insert("topP".to_string(), v.clone());
    }
    if let Some(v) = openai_payload.get("max_tokens") {
        generation_config.insert("maxOutputTokens".to_string(), v.clone());
    }
    if let Some(v) = openai_payload.get("stop") {
        generation_config.insert("stopSequences".to_string(), v.clone());
    }

    if !generation_config.is_empty() {
        out["generationConfig"] = Value::Object(generation_config);
    }

    if !system_parts.is_empty() {
        out["systemInstruction"] = json!({"parts": system_parts});
    }

    Ok(out)
}

pub fn build_gemini_upstream_url(base_url: &str, model: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.contains(":generateContent") {
        return base.to_string();
    }
    if base.contains("/models/") {
        if base.contains(':') {
            return base.to_string();
        }
        return format!("{}:generateContent", base);
    }
    format!("{}/v1beta/models/{}:generateContent", base, model)
}

fn map_gemini_finish_reason(reason: &str) -> &str {
    match reason {
        "STOP" => "stop",
        "MAX_TOKENS" => "length",
        "SAFETY" => "content_filter",
        "RECITATION" => "content_filter",
        _ => "stop",
    }
}

pub fn gemini_to_openai_chat_completion(gemini_body: &Value, model: &str) -> Value {
    let text = gemini_body
        .get("candidates")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|cand| cand.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(|v| v.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let finish_reason = gemini_body
        .get("candidates")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|cand| cand.get("finishReason"))
        .and_then(|v| v.as_str())
        .map(map_gemini_finish_reason)
        .unwrap_or("stop");

    let prompt_tokens = gemini_body
        .get("usageMetadata")
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion_tokens = gemini_body
        .get("usageMetadata")
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_tokens = gemini_body
        .get("usageMetadata")
        .and_then(|u| u.get("totalTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(prompt_tokens + completion_tokens);

    json!({
        "id": format!("chatcmpl-gemini-{}", Utc::now().timestamp_millis()),
        "object": "chat.completion",
        "created": Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": text
            },
            "finish_reason": finish_reason
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": total_tokens
        }
    })
}

pub fn build_openai_sse_from_completion(completion: &Value) -> String {
    let id = completion
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("chatcmpl-gemini");
    let created = completion
        .get("created")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| Utc::now().timestamp());
    let model = completion
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gemini");
    let text = completion
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let first = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": Value::Null}]
    });
    let second = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": "stop"}]
    });

    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        first,
        second
    )
}
