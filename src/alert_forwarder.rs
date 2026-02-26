//! Webhook transformer: convert external service payloads (Vercel, Supabase, Upstash, custom)
//! into Mattermost messages and forward them to a Mattermost incoming webhook URL.
//!
//! Route: `POST /webhooks/{source}` — source is one of `vercel`, `supabase`, `upstash`, `custom`.
//! Config: `channels_config.mattermost.alerts_incoming_webhook_url`.
//! Auth: set `channels_config.webhook.secret` to require `X-Webhook-Secret` on all alert endpoints.

use anyhow::{Context, Result};
use std::sync::LazyLock;
use std::time::Duration;

/// Shared HTTP client for forwarding to Mattermost.
/// Reusing the client enables TCP/TLS connection pooling.
/// Redirects are disabled: a POST should land exactly where configured.
static MATTERMOST_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build Mattermost HTTP client")
});

/// Conservative message length limit — below Mattermost's 16 383-character hard cap.
const MATTERMOST_MAX_TEXT_LEN: usize = 16_000;

/// Maximum pretty-printed JSON size embedded in a `custom` code block.
const CODE_BLOCK_MAX: usize = 4_000;

/// Strip Mattermost-markdown-sensitive characters from an external payload field.
///
/// Removes: `@` (mention injection), `[`, `]`, `(`, `)` (link injection),
/// `*`, `~`, `` ` `` (formatting breakout), `#` (heading injection),
/// `>` (blockquote injection).
/// Truncates to `max_len` Unicode scalar values.
fn sanitize_field(input: &str, max_len: usize) -> String {
    input
        .chars()
        .filter(|c| !matches!(c, '@' | '[' | ']' | '(' | ')' | '*' | '~' | '`' | '#' | '>'))
        .take(max_len)
        .collect()
}

/// Validate and normalise a URL from an external payload before embedding it as a link.
///
/// - Bare hostnames (no `://`) get `https://` prepended (Vercel sends bare hostnames).
/// - `http://` and `https://` URLs pass through unchanged.
/// - Any URL containing control characters (newlines, CR, NUL) is rejected to prevent
///   markdown line-injection attacks.
/// - Any other scheme (e.g. `javascript:`, `ftp://`) is rejected — returns `None`.
fn safe_http_url(url: &str) -> Option<String> {
    // Reject control characters — newlines break markdown formatting and enable injection.
    if url.chars().any(|c| c.is_control()) {
        tracing::warn!("Rejecting URL with control characters from webhook payload");
        return None;
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        Some(url.to_owned())
    } else if !url.contains(':') {
        // Bare hostname (e.g. Vercel's `deployment.url` field) — safe to prepend https://.
        Some(format!("https://{url}"))
    } else {
        // Any other scheme indicator (javascript:, ftp://, etc.) — reject.
        tracing::warn!("Rejecting non-HTTP URL from webhook payload");
        None
    }
}

/// Truncate a string to at most `max_bytes` bytes, always on a valid UTF-8 char boundary.
///
/// Using a raw byte-index slice on a multi-byte string panics if the index falls inside a
/// multi-byte code point. This helper walks backwards from `max_bytes` to find the nearest
/// safe boundary, preventing a remotely-triggerable panic.
fn truncate_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Transform a Vercel deployment webhook payload into a Mattermost message.
pub fn transform_vercel(body: &serde_json::Value) -> String {
    // Sanitize before matching so the catch-all `other` arm is already clean.
    let event_type = sanitize_field(body["type"].as_str().unwrap_or("unknown"), 64);
    let project_raw = body["payload"]["project"]["name"]
        .as_str()
        .or_else(|| body["payload"]["deployment"]["name"].as_str())
        .unwrap_or_else(|| {
            tracing::warn!("Vercel payload missing project/deployment name");
            "unknown project"
        });
    let project = sanitize_field(project_raw, 128);

    let url_raw = body["payload"]["deployment"]["url"].as_str().unwrap_or("");
    let inspector_raw = body["payload"]["links"]["deployment"]
        .as_str()
        .or_else(|| body["payload"]["deployment"]["inspectorUrl"].as_str())
        .unwrap_or("");
    let commit_raw = body["payload"]["deployment"]["meta"]["githubCommitMessage"]
        .as_str()
        .unwrap_or("");
    let commit_msg = sanitize_field(commit_raw, 256);

    let (icon, status) = match event_type.as_str() {
        "deployment.succeeded" => (":white_check_mark:", "Deploy succeeded"),
        "deployment.failed" | "deployment.error" => (":x:", "Deploy failed"),
        "deployment.cancelled" => (":warning:", "Deploy cancelled"),
        "deployment.promoted" => (":rocket:", "Deploy promoted to production"),
        other => (":information_source:", other),
    };

    let mut parts = vec![format!(
        "{icon} **Vercel** \u{00b7} **{project}** \u{2014} {status}"
    )];
    if !url_raw.is_empty() {
        if let Some(safe_url) = safe_http_url(url_raw) {
            parts.push(format!("URL: {safe_url}"));
        }
    }
    if !commit_msg.is_empty() {
        parts.push(format!("Commit: _{commit_msg}_"));
    }
    if !inspector_raw.is_empty() {
        if let Some(safe_url) = safe_http_url(inspector_raw) {
            parts.push(format!("[View deployment]({safe_url})"));
        }
    }
    parts.join("\n")
}

/// Transform a Supabase webhook payload into a Mattermost message.
///
/// Handles Supabase alert webhooks, edge function errors, and generic DB events.
pub fn transform_supabase(body: &serde_json::Value) -> String {
    // Supabase alert webhooks
    if let Some(alert_raw) = body["alert_name"].as_str() {
        let alert_name = sanitize_field(alert_raw, 128);
        let project_raw = body["project_ref"].as_str().unwrap_or_else(|| {
            tracing::warn!("Supabase alert payload missing project_ref");
            "unknown"
        });
        let project = sanitize_field(project_raw, 64);
        let message = sanitize_field(body["message"].as_str().unwrap_or(""), 512);
        let mut line =
            format!(":warning: **Supabase** \u{00b7} **{project}** \u{2014} Alert: {alert_name}");
        if !message.is_empty() {
            line.push('\n');
            line.push_str(&message);
        }
        return line;
    }

    // Supabase edge function error
    if let Some(error_raw) = body["error"].as_str() {
        let error = sanitize_field(error_raw, 256);
        let function_raw = body["function_name"].as_str().unwrap_or_else(|| {
            tracing::warn!("Supabase edge function error payload missing function_name");
            "unknown function"
        });
        let function = sanitize_field(function_raw, 128);
        return format!(":x: **Supabase** \u{00b7} Edge function **{function}** error: {error}");
    }

    // Generic DB event (from Supabase database webhooks via pg_net / Edge Functions)
    let table = sanitize_field(body["table"].as_str().unwrap_or("unknown"), 64);
    let schema = sanitize_field(body["schema"].as_str().unwrap_or("public"), 64);
    let event_type = sanitize_field(body["type"].as_str().unwrap_or("event"), 32);
    format!(":floppy_disk: **Supabase** \u{00b7} `{schema}.{table}` \u{2014} {event_type} event")
}

/// Transform an Upstash webhook payload into a Mattermost message.
pub fn transform_upstash(body: &serde_json::Value) -> String {
    // Sanitize before matching so the catch-all `other` arm is already clean.
    let event = sanitize_field(body["event"].as_str().unwrap_or("event"), 64);
    let resource_raw = body["database_id"]
        .as_str()
        .or_else(|| body["queue_name"].as_str())
        .unwrap_or_else(|| {
            tracing::warn!("Upstash payload missing database_id/queue_name");
            "unknown"
        });
    let resource = sanitize_field(resource_raw, 64);

    let (icon, label) = match event.as_str() {
        "rate_limit_exceeded" => (":warning:", "Rate limit exceeded"),
        "circuit_breaker_open" => (":red_circle:", "Circuit breaker opened"),
        "circuit_breaker_close" => (":large_green_circle:", "Circuit breaker closed"),
        "dlq_message_received" => (
            ":skull_and_crossbones:",
            "Dead letter queue message received",
        ),
        other => (":information_source:", other),
    };

    let msg_raw = body["message"]
        .as_str()
        .or_else(|| body["details"]["message"].as_str())
        .unwrap_or("");
    let msg = sanitize_field(msg_raw, 512);

    let mut line = format!("{icon} **Upstash** \u{00b7} **{resource}** \u{2014} {label}");
    if !msg.is_empty() {
        line.push('\n');
        line.push_str(&msg);
    }
    line
}

/// Transform a custom webhook payload into a Mattermost message.
///
/// If the payload has a top-level `"message"` string, it is used as the message body.
/// Otherwise the entire JSON is rendered as a truncated code block.
pub fn transform_custom(body: &serde_json::Value) -> String {
    if let Some(msg_raw) = body["message"].as_str() {
        let source = sanitize_field(body["source"].as_str().unwrap_or("custom"), 64);
        let msg = sanitize_field(msg_raw, 1024);
        return format!(":incoming_envelope: **{source}** \u{2014} {msg}");
    }

    // serde_json::to_string_pretty on a Value writes to an in-memory Vec and is infallible.
    let pretty = serde_json::to_string_pretty(body)
        .expect("serializing a serde_json::Value to string is infallible");

    let body_text = if pretty.len() > CODE_BLOCK_MAX {
        tracing::warn!(
            "Custom webhook payload is {} bytes; truncating to {} for Mattermost",
            pretty.len(),
            CODE_BLOCK_MAX
        );
        format!("{}...(truncated)", truncate_bytes(&pretty, CODE_BLOCK_MAX))
    } else {
        pretty
    };

    // Escape triple-backtick sequences so attacker-controlled values cannot break out of
    // the fenced code block and inject arbitrary Mattermost markdown.
    let body_text = body_text.replace("```", "\\`\\`\\`");

    format!(":incoming_envelope: **Custom webhook**\n```json\n{body_text}\n```")
}

/// Forward a Mattermost message to an incoming webhook URL.
///
/// The `url` is a Mattermost incoming webhook URL (Integrations → Incoming Webhooks).
/// Uses a shared client with a 10 s response timeout and 5 s connect timeout.
pub async fn forward_to_mattermost(url: &str, text: &str) -> Result<()> {
    let text = if text.len() > MATTERMOST_MAX_TEXT_LEN {
        tracing::warn!(
            "Alert message is {} bytes; truncating to {} for Mattermost",
            text.len(),
            MATTERMOST_MAX_TEXT_LEN
        );
        truncate_bytes(text, MATTERMOST_MAX_TEXT_LEN)
    } else {
        text
    };

    let payload = serde_json::json!({ "text": text });
    let resp = MATTERMOST_CLIENT
        .post(url)
        .json(&payload)
        .send()
        .await
        .context("Failed to reach Mattermost incoming webhook")?;

    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));
        anyhow::bail!("Mattermost incoming webhook returned {status}: {body}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vercel_succeeded_formats_correctly() {
        let payload = json!({
            "type": "deployment.succeeded",
            "payload": {
                "project": {"name": "zeroclaw-dashboard"},
                "deployment": {
                    "url": "zeroclaw-dashboard-abc.vercel.app",
                    "meta": {"githubCommitMessage": "fix: handle edge case"}
                },
                "links": {"deployment": "https://vercel.com/team/zeroclaw-dashboard/abc"}
            }
        });
        let msg = transform_vercel(&payload);
        assert!(
            msg.contains("zeroclaw-dashboard"),
            "should include project name"
        );
        assert!(msg.contains("Deploy succeeded"), "should indicate success");
        assert!(
            msg.contains("fix: handle edge case"),
            "should include commit message"
        );
        assert!(
            msg.contains("zeroclaw-dashboard-abc.vercel.app"),
            "should include URL"
        );
    }

    #[test]
    fn vercel_failed_uses_failure_status() {
        let payload = json!({
            "type": "deployment.failed",
            "payload": {"project": {"name": "zeroclaw-api"}, "deployment": {}, "links": {}}
        });
        let msg = transform_vercel(&payload);
        assert!(msg.contains("Deploy failed"));
        assert!(msg.contains("zeroclaw-api"));
    }

    #[test]
    fn vercel_unknown_type_passes_through() {
        let payload = json!({
            "type": "deployment.queued",
            "payload": {"project": {"name": "proj"}, "deployment": {}, "links": {}}
        });
        let msg = transform_vercel(&payload);
        assert!(msg.contains("deployment.queued"));
    }

    #[test]
    fn vercel_prepends_https_to_bare_hostname() {
        let payload = json!({
            "type": "deployment.succeeded",
            "payload": {
                "project": {"name": "proj"},
                "deployment": {"url": "myapp-abc.vercel.app"},
                "links": {}
            }
        });
        let msg = transform_vercel(&payload);
        assert!(
            msg.contains("https://myapp-abc.vercel.app"),
            "bare hostname should get https://"
        );
        assert!(
            !msg.contains("https://https://"),
            "must not double-prepend scheme"
        );
    }

    #[test]
    fn vercel_rejects_non_http_inspector_url() {
        let payload = json!({
            "type": "deployment.succeeded",
            "payload": {
                "project": {"name": "proj"},
                "deployment": {},
                "links": {"deployment": "javascript:alert(1)"}
            }
        });
        let msg = transform_vercel(&payload);
        assert!(
            !msg.contains("javascript:"),
            "non-http scheme must be rejected"
        );
        assert!(
            !msg.contains("View deployment"),
            "malicious link must be dropped"
        );
    }

    #[test]
    fn vercel_strips_at_mentions_from_commit_msg() {
        let payload = json!({
            "type": "deployment.succeeded",
            "payload": {
                "project": {"name": "proj"},
                "deployment": {"meta": {"githubCommitMessage": "@channel urgent fix"}},
                "links": {}
            }
        });
        let msg = transform_vercel(&payload);
        assert!(!msg.contains("@channel"), "@ mentions must be stripped");
        assert!(
            msg.contains("channel urgent fix"),
            "text after @ must be preserved"
        );
    }

    #[test]
    fn supabase_alert_formats_correctly() {
        let payload = json!({
            "alert_name": "High CPU",
            "project_ref": "zeroclaw-project",
            "message": "CPU above 80%"
        });
        let msg = transform_supabase(&payload);
        assert!(msg.contains("High CPU"));
        assert!(msg.contains("zeroclaw-project"));
        assert!(msg.contains("CPU above 80%"));
    }

    #[test]
    fn supabase_edge_function_error_formats_correctly() {
        let payload = json!({
            "error": "timeout after 5000ms",
            "function_name": "process-webhook"
        });
        let msg = transform_supabase(&payload);
        assert!(msg.contains("process-webhook"));
        assert!(msg.contains("timeout after 5000ms"));
    }

    #[test]
    fn supabase_db_event_formats_correctly() {
        let payload = json!({
            "type": "INSERT",
            "table": "issues",
            "schema": "public"
        });
        let msg = transform_supabase(&payload);
        assert!(msg.contains("public.issues"));
        assert!(msg.contains("INSERT"));
    }

    #[test]
    fn upstash_rate_limit_formats_correctly() {
        let payload = json!({
            "event": "rate_limit_exceeded",
            "database_id": "zeroclaw-cache",
            "message": "100 req/s limit hit"
        });
        let msg = transform_upstash(&payload);
        assert!(msg.contains("zeroclaw-cache"));
        assert!(msg.contains("Rate limit exceeded"));
        assert!(msg.contains("100 req/s limit hit"));
    }

    #[test]
    fn upstash_unknown_event_passes_through() {
        let payload = json!({"event": "custom_event", "database_id": "db"});
        let msg = transform_upstash(&payload);
        assert!(msg.contains("custom_event"));
    }

    #[test]
    fn custom_with_message_field() {
        let payload = json!({"message": "build finished", "source": "jenkins"});
        let msg = transform_custom(&payload);
        assert!(msg.contains("jenkins"));
        assert!(msg.contains("build finished"));
    }

    #[test]
    fn custom_without_message_falls_back_to_code_block() {
        let payload = json!({"foo": "bar", "count": 42});
        let msg = transform_custom(&payload);
        assert!(msg.contains("Custom webhook"));
        assert!(msg.contains("foo"));
    }

    #[test]
    fn custom_strips_at_mentions() {
        let payload = json!({
            "message": "@here urgent security alert",
            "source": "prod-monitor"
        });
        let msg = transform_custom(&payload);
        assert!(!msg.contains("@here"), "@ mentions must be stripped");
        assert!(msg.contains("here urgent security alert"));
    }

    #[test]
    fn custom_truncates_large_json_payload() {
        let large_value: serde_json::Value =
            serde_json::from_str(&format!("{{\"data\": \"{}\"}}", "x".repeat(5000))).unwrap();
        let msg = transform_custom(&large_value);
        assert!(
            msg.contains("truncated"),
            "large payloads must be truncated"
        );
    }

    #[test]
    fn safe_http_url_accepts_https() {
        assert_eq!(
            safe_http_url("https://example.com"),
            Some("https://example.com".to_owned())
        );
    }

    #[test]
    fn safe_http_url_prepends_https_to_bare_host() {
        assert_eq!(
            safe_http_url("myapp.vercel.app"),
            Some("https://myapp.vercel.app".to_owned())
        );
    }

    #[test]
    fn safe_http_url_rejects_javascript_scheme() {
        assert_eq!(safe_http_url("javascript:alert(1)"), None);
    }

    #[test]
    fn sanitize_field_strips_mention_and_link_chars() {
        let input = "@channel [click here](http://evil.com) **bold**";
        let out = sanitize_field(input, 512);
        assert!(!out.contains('@'));
        assert!(!out.contains('['));
        assert!(!out.contains('*'));
    }

    #[test]
    fn sanitize_field_strips_heading_and_blockquote_chars() {
        let out = sanitize_field("# Heading\n> Quote", 512);
        assert!(!out.contains('#'));
        assert!(!out.contains('>'));
    }

    #[test]
    fn truncate_bytes_stays_on_utf8_boundary() {
        // "日本語" is 3 chars × 3 bytes each = 9 bytes total.
        // Truncating at 5 bytes would land mid-char; should back up to 3 (end of "日").
        let s = "日本語";
        assert_eq!(s.len(), 9);
        let result = truncate_bytes(s, 5);
        assert!(s.is_char_boundary(result.len()));
        assert_eq!(result, "日"); // 3 bytes, first valid boundary ≤ 5
    }

    #[test]
    fn truncate_bytes_exact_boundary_unchanged() {
        let s = "abc";
        assert_eq!(truncate_bytes(s, 3), "abc");
        assert_eq!(truncate_bytes(s, 10), "abc");
    }

    #[test]
    fn truncate_bytes_zero_max_returns_empty() {
        assert_eq!(truncate_bytes("hello", 0), "");
    }

    #[test]
    fn safe_http_url_rejects_control_characters() {
        assert_eq!(
            safe_http_url("https://example.com/\nX-Injected: header"),
            None
        );
        assert_eq!(safe_http_url("https://example.com/\0null"), None);
        assert_eq!(safe_http_url("https://example.com/\rpath"), None);
    }
}
