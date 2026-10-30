/// Returns Codex CLI configuration flags to route requests through contextWatcher's authenticated Responses proxy.
pub fn codex_proxy_config_args(proxy_port: u16) -> Vec<String> {
    [
        "model_provider=\"context_watcher\"".to_owned(),
        "model_providers.context_watcher.name=\"contextWatcher\"".to_owned(),
        format!(
            "model_providers.context_watcher.base_url=\"http://127.0.0.1:{proxy_port}/openai/v1\""
        ),
        "model_providers.context_watcher.wire_api=\"responses\"".to_owned(),
        "model_providers.context_watcher.requires_openai_auth=true".to_owned(),
        "model_providers.context_watcher.env_key=\"OPENAI_API_KEY\"".to_owned(),
        "model_providers.context_watcher.env_http_headers={\"x-cw-session-id\"=\"CW_SESSION_ID\",\"x-cw-control-token\"=\"CW_CONTROL_TOKEN\"}".to_owned(),
        "shell_environment_policy.filters.CW_CONTROL_TOKEN=\"exclude\"".to_owned(),
    ]
    .into_iter()
    .flat_map(|setting| ["--config".to_owned(), setting])
    .collect()
}

/// Generates isolated models.json content for OMP/Pi pointing to contextWatcher's proxy.
pub fn omp_proxy_models_config(proxy_port: u16, model_id: &str) -> serde_json::Value {
    serde_json::json!({
        "providers": {
            "cw-proxy": {
                "baseUrl": format!("http://127.0.0.1:{proxy_port}/openai/v1"),
                "api": "openai-completions",
                "apiKey": "cw-authenticated",
                "models": [{
                    "id": model_id,
                    "name": format!("contextWatcher ({model_id})"),
                    "contextWindow": 131072,
                    "maxTokens": 4096
                }]
            }
        }
    })
}
