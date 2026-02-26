//! Webhook transformer: convert external service payloads (Vercel, Supabase, Upstash, custom)
//! into Mattermost messages and forward them to a Mattermost incoming webhook URL.
//!
//! Route: `POST /webhooks/{source}` — source is one of `vercel`, `supabase`, `upstash`, `custom`.
//! Config: `channels_config.mattermost.alerts_incoming_webhook_url`.

use anyhow::{Context, Result};

/// Transform a Vercel deployment webhook payload into a Mattermost message.
pub fn transform_vercel(body: &serde_json::Value) -> String {
    let event_type = body["type"].as_str().unwrap_or("unknown");
    let project = body["payload"]["project"]["name"]
        .as_str()
        .or_else(|| body["payload"]["deployment"]["name"].as_str())
        .unwrap_or("unknown project");
    let url = body["payload"]["deployment"]["url"].as_str().unwrap_or("");
    let inspector = body["payload"]["links"]["deployment"]
        .as_str()
        .or_else(|| body["payload"]["deployment"]["inspectorUrl"].as_str())
        .unwrap_or("");
    let commit_msg = body["payload"]["deployment"]["meta"]["githubCommitMessage"]
        .as_str()
        .unwrap_or("");

    let (icon, status) = match event_type {
        "deployment.succeeded" => (":white_check_mark:", "Deploy succeeded"),
        "deployment.failed" | "deployment.error" => (":x:", "Deploy failed"),
        "deployment.cancelled" => (":warning:", "Deploy cancelled"),
        "deployment.promoted" => (":rocket:", "Deploy promoted to production"),
        other => (":information_source:", other),
    };

    let mut parts = vec![format!(
        "{icon} **Vercel** \u{00b7} **{project}** \u{2014} {status}"
    )];
    if !url.is_empty() {
        parts.push(format!("URL: https://{url}"));
    }
    if !commit_msg.is_empty() {
        parts.push(format!("Commit: _{commit_msg}_"));
    }
    if !inspector.is_empty() {
        parts.push(format!("[View deployment]({inspector})"));
    }
    parts.join("\n")
}

/// Transform a Supabase webhook payload into a Mattermost message.
///
/// Handles Supabase alert webhooks, edge function errors, and generic DB events.
pub fn transform_supabase(body: &serde_json::Value) -> String {
    // Supabase alert webhooks (e.g. from project monitoring integrations)
    if let Some(alert_name) = body["alert_name"].as_str() {
        let project = body["project_ref"].as_str().unwrap_or("unknown");
        let message = body["message"].as_str().unwrap_or("");
        let mut line =
            format!(":warning: **Supabase** \u{00b7} **{project}** \u{2014} Alert: {alert_name}");
        if !message.is_empty() {
            line.push('\n');
            line.push_str(message);
        }
        return line;
    }

    // Supabase edge function error
    if let Some(error) = body["error"].as_str() {
        let function = body["function_name"].as_str().unwrap_or("unknown function");
        return format!(":x: **Supabase** \u{00b7} Edge function **{function}** error: {error}");
    }

    // Generic DB event (from Supabase database webhooks via pg_net / Edge Functions)
    let table = body["table"].as_str().unwrap_or("unknown");
    let schema = body["schema"].as_str().unwrap_or("public");
    let event_type = body["type"].as_str().unwrap_or("event");
    format!(":floppy_disk: **Supabase** \u{00b7} `{schema}.{table}` \u{2014} {event_type} event")
}

/// Transform an Upstash webhook payload into a Mattermost message.
pub fn transform_upstash(body: &serde_json::Value) -> String {
    let event = body["event"].as_str().unwrap_or("event");
    let resource = body["database_id"]
        .as_str()
        .or_else(|| body["queue_name"].as_str())
        .unwrap_or("unknown");

    let (icon, label) = match event {
        "rate_limit_exceeded" => (":warning:", "Rate limit exceeded"),
        "circuit_breaker_open" => (":red_circle:", "Circuit breaker opened"),
        "circuit_breaker_close" => (":large_green_circle:", "Circuit breaker closed"),
        "dlq_message_received" => (
            ":skull_and_crossbones:",
            "Dead letter queue message received",
        ),
        other => (":information_source:", other),
    };

    let msg = body["message"]
        .as_str()
        .or_else(|| body["details"]["message"].as_str())
        .unwrap_or("");

    let mut line = format!("{icon} **Upstash** \u{00b7} **{resource}** \u{2014} {label}");
    if !msg.is_empty() {
        line.push('\n');
        line.push_str(msg);
    }
    line
}

/// Transform a custom webhook payload into a Mattermost message.
///
/// If the payload has a top-level `"message"` string, it is used as the message body.
/// Otherwise the entire JSON is rendered as a code block.
pub fn transform_custom(body: &serde_json::Value) -> String {
    if let Some(msg) = body["message"].as_str() {
        let source = body["source"].as_str().unwrap_or("custom");
        return format!(":incoming_envelope: **{source}** \u{2014} {msg}");
    }
    let pretty = serde_json::to_string_pretty(body).unwrap_or_else(|_| body.to_string());
    format!(":incoming_envelope: **Custom webhook**\n```json\n{pretty}\n```")
}

/// Forward a Mattermost message to an incoming webhook URL.
///
/// The `url` is a Mattermost incoming webhook URL (Integrations → Incoming Webhooks).
pub async fn forward_to_mattermost(url: &str, text: &str) -> Result<()> {
    let payload = serde_json::json!({ "text": text });
    let resp = reqwest::Client::new()
        .post(url)
        .json(&payload)
        .send()
        .await
        .context("Failed to reach Mattermost incoming webhook")?;

    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
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
}
