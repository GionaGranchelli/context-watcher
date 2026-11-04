use crate::PinPolicyAction;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::Write;

fn endpoint(base: &str, session: &str) -> Result<reqwest::Url> {
    if !cw_core::control::is_valid_session_id(session) {
        bail!("Invalid session ID");
    }
    let mut url = reqwest::Url::parse(base).context("Invalid daemon URL")?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if !loopback
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("Use a bare loopback HTTP(S) daemon URL");
    }
    url.set_path(&format!("/api/v1/sessions/{session}/pin-context-policy"));
    Ok(url)
}

fn validate_preview(body: Value, session: &str) -> Result<Value> {
    if body["session_id"].as_str() != Some(session)
        || !body["enabled"].is_boolean()
        || !body["session_terminated"].is_boolean()
        || !body["reinforcement_supported"].is_boolean()
        || body["risk_warning"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
        || body["activation_scope"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
        || !body["preview_basis"].is_string()
        || !body["stored_pins"].is_array()
        || !body["automatic_generation"]["may_generate_after_opt_in"].is_boolean()
        || !body["automatic_generation"]["auto_cache_safe_pin"].is_boolean()
        || !body["automatic_generation"]["auto_heal_low_health"].is_boolean()
        || !body["automatic_generation"]["heal_strategy"].is_string()
    {
        bail!("Invalid or unavailable policy preview; consent state is unconfirmed");
    }
    for pin in body["stored_pins"]
        .as_array()
        .context("Missing stored pins")?
    {
        if !pin["rule_id"].is_string()
            || !matches!(
                pin.get("eligible_in_latest_request_preview_after_opt_in"),
                Some(Value::Null | Value::Bool(_))
            )
            || !pin["label"].is_string()
            || !pin["content"].is_string()
            || !matches!(
                pin["status"].as_str(),
                Some("draft" | "committed" | "disabled")
            )
        {
            bail!("Invalid stored pin in policy preview");
        }
    }
    Ok(body)
}

async fn response_body(response: reqwest::Response, session: &str) -> Result<Value> {
    if !response.status().is_success() {
        bail!(
            "Policy API returned HTTP {}; consent state is unconfirmed",
            response.status().as_u16()
        );
    }
    validate_preview(
        response
            .json()
            .await
            .context("Invalid policy API JSON; consent state is unconfirmed")?,
        session,
    )
}

pub async fn run(session: &str, base: &str, action: PinPolicyAction) -> Result<()> {
    let url = endpoint(base, session)?;
    let token = cw_core::control::control_token(false)
        .context("Use the existing daemon control credential")?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    if !matches!(action, PinPolicyAction::Disable) {
        let preview = response_body(
            client
                .get(url.clone())
                .header("x-cw-control-token", &token)
                .send()
                .await
                .context("Policy API unavailable; consent state is unconfirmed")?,
            session,
        )
        .await?;
        println!("Activation preview (no consent changed):");
        println!("{}", serde_json::to_string_pretty(&preview)?);
        println!(
            "{}",
            preview["risk_warning"]
                .as_str()
                .context("Missing risk warning")?
        );
        println!(
            "Authorization and preview eligibility are not proof of application to a provider request. Measured savings unavailable."
        );
        std::io::stdout().flush()?;
        if let PinPolicyAction::Enable { acknowledge_risk } = action {
            if preview["session_terminated"] == true {
                bail!("Cannot enable reinforcement for a terminated session");
            }
            if preview["reinforcement_supported"] == false {
                bail!(
                    "Reinforcement is unsupported for this session provider (Gemini remains unqualified)"
                );
            }
            if !acknowledge_risk {
                bail!(
                    "Review the preview and warning, then use enable --acknowledge-risk to authorize session reinforcement"
                );
            }
        } else {
            return Ok(());
        }
    }
    let enable = matches!(action, PinPolicyAction::Enable { .. });
    let body = response_body(
        client
            .post(url)
            .header("x-cw-control-token", &token)
            .json(&json!({"enabled":enable,"acknowledge_risk":enable}))
            .send()
            .await
            .context("Policy API unavailable; consent update is unconfirmed")?,
        session,
    )
    .await?;
    if body["enabled"].as_bool() != Some(enable) {
        bail!("Policy API did not confirm the requested consent state");
    }
    println!(
        "{}",
        if enable {
            "Reinforcement authorized for this session. This does not prove provider-request application."
        } else {
            "Reinforcement consent revoked. Stored rules were not deleted."
        }
    );
    println!("Confirmed policy state:");
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_bare_loopback_control_urls_are_accepted() {
        for base in [
            "http://127.0.0.1:4400",
            "http://localhost:4400",
            "http://[::1]:4400",
        ] {
            assert!(endpoint(base, "session-a").is_ok());
        }
        for base in [
            "https://example.com",
            "http://192.168.1.1:4400",
            "http://token@localhost:4400",
            "http://localhost:4400/path",
            "http://localhost:4400?key=value",
            "http://localhost:4400#fragment",
        ] {
            assert!(endpoint(base, "session-a").is_err());
        }
        assert!(endpoint("http://localhost:4400", "bad/id").is_err());
    }
}
