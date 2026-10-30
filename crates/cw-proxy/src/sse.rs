use serde_json::Value;

pub fn extract_cached_tokens_from_json(resp: &Value) -> Option<u32> {
    // Support llama.cpp `timings.cache_n`
    if let Some(cache_n) = resp
        .get("timings")
        .and_then(|t| t.get("cache_n"))
        .and_then(|v| v.as_u64())
    {
        return Some(cache_n as u32);
    }

    // Support Anthropic SSE `message_start.message.usage`
    let usage = resp
        .get("usage")
        .or_else(|| resp.get("usageMetadata"))
        .or_else(|| resp.get("message").and_then(|m| m.get("usage")))?;

    usage
        .get("cache_read_input_tokens")
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
        })
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
        })
        .or_else(|| usage.get("cachedContentTokenCount"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
}

pub fn extract_cached_tokens_from_sse(chunk_text: &str) -> Option<u32> {
    for line in chunk_text.lines() {
        if let Some(data) = line.strip_prefix("data: ")
            && let Ok(val) = serde_json::from_str::<Value>(data)
            && let Some(c) = extract_cached_tokens_from_json(&val)
        {
            return Some(c);
        }
    }
    None
}

#[derive(Default, Debug)]
pub struct SseChunkAccumulator {
    buffer: String,
}

impl SseChunkAccumulator {
    pub fn push_chunk(&mut self, bytes: &[u8]) -> Option<u32> {
        let text = String::from_utf8_lossy(bytes);
        self.buffer.push_str(&text);
        if let Some(idx) = self.buffer.rfind('\n') {
            let complete = &self.buffer[..=idx];
            let cached = extract_cached_tokens_from_sse(complete);
            self.buffer.drain(..=idx);
            return cached;
        }
        None
    }

    pub fn flush_cached(&mut self) -> Option<u32> {
        let res = extract_cached_tokens_from_sse(&self.buffer);
        self.buffer.clear();
        res
    }
}
