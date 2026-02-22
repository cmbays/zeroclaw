//! Inbound webhook listener for Linear and GitHub events.
//!
//! Starts a standalone axum HTTP server on `[linear].webhook_port` and dispatches
//! verified payloads to the appropriate handler. Each POST handler:
//! 1. Reads the raw body.
//! 2. Verifies the HMAC-SHA256 signature against `[linear].webhook_signing_secret`.
//! 3. Parses the JSON payload.
//! 4. Calls the relevant `slack_ops` function.

use crate::config::Config;
use crate::tools::slack_ops;
use anyhow::{bail, Result};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;

// ── Shared state ──────────────────────────────────────────────────────────────

#[derive(Clone)]
struct WebhookState {
    bot_token: String,
    signing_secret: Option<String>,
}

// ── Signature verification ────────────────────────────────────────────────────

/// Verify a `linear-signature` or `X-Hub-Signature-256` HMAC-SHA256 header.
///
/// `header_value` should be the raw header value (Linear: bare hex, GitHub: `sha256=<hex>`).
/// Returns `Ok(())` when the signature is valid or no secret is configured.
/// Returns `Err` if a secret is present but the signature is missing or invalid.
fn verify_hmac(body: &[u8], header_value: Option<&str>, secret: Option<&str>) -> Result<()> {
    let Some(secret) = secret else {
        return Ok(()); // no secret configured — accept all
    };

    let provided = header_value
        .ok_or_else(|| anyhow::anyhow!("webhook: signature header missing"))?
        .trim_start_matches("sha256=");

    let expected = compute_hmac(body, secret);
    if !constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
        bail!("webhook: signature mismatch");
    }
    Ok(())
}

fn compute_hmac(body: &[u8], secret: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

// ── Route handlers ────────────────────────────────────────────────────────────

/// POST /webhook/linear — receive Linear project events.
async fn handle_linear(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let sig = headers
        .get("linear-signature")
        .and_then(|v| v.to_str().ok());

    if let Err(e) = verify_hmac(&body, sig, state.signing_secret.as_deref()) {
        tracing::warn!("linear webhook: {e}");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("linear webhook: invalid JSON — {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    dispatch_linear_event(&state, &payload).await;
    StatusCode::OK
}

/// POST /webhook/github — receive GitHub PR events.
async fn handle_github(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok());

    if let Err(e) = verify_hmac(&body, sig, state.signing_secret.as_deref()) {
        tracing::warn!("github webhook: {e}");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("github webhook: invalid JSON — {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    dispatch_github_event(&payload);
    StatusCode::OK
}

// ── Event dispatchers ─────────────────────────────────────────────────────────

async fn dispatch_linear_event(state: &WebhookState, payload: &serde_json::Value) {
    let action = payload.get("action").and_then(|a| a.as_str()).unwrap_or("");
    let type_ = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");

    tracing::debug!("linear webhook: type={type_} action={action}");

    // Project created → auto-create a Slack channel.
    if type_ == "Project" && action == "create" {
        on_linear_project_create(state, payload).await;
    }
}

async fn on_linear_project_create(state: &WebhookState, payload: &serde_json::Value) {
    let name = payload
        .pointer("/data/name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unnamed Project");

    let url = payload
        .pointer("/data/url")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match slack_ops::create_project_channel(&state.bot_token, name, url).await {
        Ok(ch) => tracing::info!("linear webhook: created Slack channel {ch} for '{name}'"),
        Err(e) => tracing::error!("linear webhook: failed to create channel for '{name}': {e}"),
    }
}

fn dispatch_github_event(payload: &serde_json::Value) {
    let action = payload.get("action").and_then(|a| a.as_str()).unwrap_or("");
    let merged = payload
        .pointer("/pull_request/merged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    tracing::debug!("github webhook: action={action} merged={merged}");

    // PR merged → log for future lifecycle hooks (stub for now).
    if action == "closed" && merged {
        let pr_title = payload
            .pointer("/pull_request/title")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        let pr_url = payload
            .pointer("/pull_request/html_url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        tracing::info!("github webhook: PR merged — '{pr_title}' {pr_url}");
    }
}

// ── Server startup ────────────────────────────────────────────────────────────

/// Start the webhook HTTP listener. Runs until cancelled.
///
/// Requires `config.linear.webhook_port` to be `Some`. If `config.linear.webhook_signing_secret`
/// is set, all requests must carry a valid HMAC-SHA256 signature.
pub async fn run(config: &Config) -> Result<()> {
    let port = config
        .linear
        .webhook_port
        .ok_or_else(|| anyhow::anyhow!("webhook: linear.webhook_port not configured"))?;

    let bot_token = config
        .channels_config
        .slack
        .as_ref()
        .map(|s| s.bot_token.clone())
        .ok_or_else(|| anyhow::anyhow!("webhook: [channels.slack] bot_token required"))?;

    let state = Arc::new(WebhookState {
        bot_token,
        signing_secret: config.linear.webhook_signing_secret.clone(),
    });

    let app = Router::new()
        .route("/webhook/linear", post(handle_linear))
        .route("/webhook/github", post(handle_github))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("webhook: listening on {addr}");

    axum::serve(listener, app).await?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_valid_signature() {
        let body = b"hello world";
        let secret = "mysecret";
        let sig = compute_hmac(body, secret);
        assert!(verify_hmac(body, Some(&sig), Some(secret)).is_ok());
    }

    #[test]
    fn hmac_invalid_signature() {
        let body = b"hello world";
        assert!(verify_hmac(body, Some("deadbeef"), Some("mysecret")).is_err());
    }

    #[test]
    fn hmac_missing_header_with_secret() {
        let body = b"hello world";
        assert!(verify_hmac(body, None, Some("mysecret")).is_err());
    }

    #[test]
    fn hmac_no_secret_always_ok() {
        let body = b"hello world";
        assert!(verify_hmac(body, None, None).is_ok());
    }

    #[test]
    fn hmac_github_sha256_prefix_stripped() {
        let body = b"payload";
        let secret = "ghs";
        let raw = compute_hmac(body, secret);
        let prefixed = format!("sha256={raw}");
        assert!(verify_hmac(body, Some(&prefixed), Some(secret)).is_ok());
    }

    #[test]
    fn constant_time_eq_matching() {
        assert!(constant_time_eq(b"abc", b"abc"));
    }

    #[test]
    fn constant_time_eq_different_length() {
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn constant_time_eq_different_content() {
        assert!(!constant_time_eq(b"abc", b"xyz"));
    }
}
