use crate::state::DaemonState;
use axum::http::Uri;

/// Resolves the upstream target URL from the incoming request URI and daemon state.
pub fn resolve_upstream_target(state: &DaemonState, uri: &Uri) -> String {
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let routes = [
        ("/anthropic", &state.anthropic_upstream),
        ("/openai", &state.openai_upstream),
        ("/deepseek", &state.deepseek_upstream),
        ("/gemini", &state.gemini_upstream),
        ("/ollama", &state.ollama_upstream),
        ("/llamacpp", &state.llamacpp_upstream),
        ("/vllm", &state.vllm_upstream),
    ];
    let fallback = if uri.path().ends_with("/v1/messages") {
        &state.anthropic_upstream
    } else if path.contains("generateContent") {
        &state.gemini_upstream
    } else {
        &state.openai_upstream
    };
    let (base, suffix) = routes
        .iter()
        .find_map(|(prefix, base)| {
            path.strip_prefix(prefix)
                .filter(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('?'))
                .map(|rest| (*base, if rest.is_empty() { "/" } else { rest }))
        })
        .unwrap_or((fallback, path));
    format!("{}{suffix}", base.trim_end_matches('/'))
}
