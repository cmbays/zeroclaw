//! Slack channel lifecycle operations for the webhook handler.
//!
//! Provides helpers to create project-scoped channels and detect
//! bidirectional links between Slack channels and Linear projects.

use anyhow::{bail, Result};

// ── Slug helpers ──────────────────────────────────────────────────────────────

/// Convert a Linear project name into a Slack-safe channel slug.
///
/// Rules: lowercase, replace non-alphanumeric runs with `-`, prefix `prj-`,
/// truncate to 80 chars (Slack limit).
///
/// `"Auth Refactor"` → `"prj-auth-refactor"`, `"Q1/2026 -- Infra"` → `"prj-q1-2026-infra"`
pub fn project_name_to_slack_slug(name: &str) -> anyhow::Result<String> {
    let raw: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse consecutive dashes, strip leading/trailing dashes.
    let collapsed = raw
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if collapsed.is_empty() {
        anyhow::bail!("project name '{name}' produces an empty channel slug");
    }

    let slug = format!("prj-{collapsed}");
    Ok(slug.chars().take(80).collect())
}

// ── Channel lifecycle ─────────────────────────────────────────────────────────

fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default()
}

/// Create a `#prj-<slug>` Slack channel for a new Linear project.
///
/// Steps:
/// 1. Convert `project_name` → slug → channel name.
/// 2. Call `conversations.create`.
/// 3. Set channel topic with the Linear project URL.
/// 4. Post a creation notice in the new channel.
///
/// Returns the Slack channel ID on success.
pub async fn create_project_channel(
    bot_token: &str,
    project_name: &str,
    linear_url: &str,
) -> Result<String> {
    let client = build_http_client();
    let channel_name = project_name_to_slack_slug(project_name)?;

    // 1. Create the channel.
    let create_resp: serde_json::Value = client
        .post("https://slack.com/api/conversations.create")
        .bearer_auth(bot_token)
        .json(&serde_json::json!({ "name": channel_name, "is_private": false }))
        .send()
        .await?
        .json()
        .await?;

    if create_resp.get("ok") != Some(&serde_json::Value::Bool(true)) {
        let err = create_resp
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("unknown");

        // name_taken is recoverable — just look up the existing channel.
        if err == "name_taken" {
            tracing::warn!("slack_ops: channel #{channel_name} already exists; skipping create");
            return find_channel_id_by_name(bot_token, &channel_name).await;
        }

        bail!("conversations.create failed for #{channel_name}: {err}");
    }

    let channel_id = create_resp
        .pointer("/channel/id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("conversations.create response missing channel.id"))?
        .to_string();

    // 2. Set topic to include bidirectional Linear link.
    let topic = format!("Linear project: {linear_url}");
    // Best-effort: topic is informational; failure doesn't block channel creation.
    let _ = client
        .post("https://slack.com/api/conversations.setTopic")
        .bearer_auth(bot_token)
        .json(&serde_json::json!({ "channel": channel_id, "topic": topic }))
        .send()
        .await;

    // 3. Post creation notice.
    let notice = format!(
        ":white_check_mark: Channel created for Linear project *{project_name}*\n{linear_url}"
    );
    // Best-effort: creation notice; failure doesn't block channel creation.
    let _ = client
        .post("https://slack.com/api/chat.postMessage")
        .bearer_auth(bot_token)
        .json(&serde_json::json!({ "channel": channel_id, "text": notice }))
        .send()
        .await;

    tracing::info!("slack_ops: created #{channel_name} ({channel_id}) for {project_name}");
    Ok(channel_id)
}

/// Resolve a channel ID by exact name match using `conversations.list`.
///
/// Used as a fallback when `name_taken` prevents channel creation.
async fn find_channel_id_by_name(bot_token: &str, name: &str) -> Result<String> {
    let client = build_http_client();
    let resp: serde_json::Value = client
        .get("https://slack.com/api/conversations.list")
        .bearer_auth(bot_token)
        .query(&[("exclude_archived", "true"), ("limit", "200")])
        .send()
        .await?
        .json()
        .await?;

    resp.get("channels")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .find_map(|ch| {
            let ch_name = ch.get("name").and_then(|n| n.as_str())?;
            let id = ch.get("id").and_then(|i| i.as_str())?;
            if ch_name == name {
                Some(id.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("could not find existing channel #{name}"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_simple_name() {
        assert_eq!(
            project_name_to_slack_slug("Auth Refactor").unwrap(),
            "prj-auth-refactor"
        );
    }

    #[test]
    fn slug_special_chars() {
        assert_eq!(
            project_name_to_slack_slug("Q1/2026 -- Infra").unwrap(),
            "prj-q1-2026-infra"
        );
    }

    #[test]
    fn slug_already_lowercase() {
        assert_eq!(
            project_name_to_slack_slug("billing").unwrap(),
            "prj-billing"
        );
    }

    #[test]
    fn slug_truncates_at_80_chars() {
        let long_name = "a".repeat(90);
        let slug = project_name_to_slack_slug(&long_name).unwrap();
        assert!(slug.len() <= 80, "slug must not exceed 80 chars");
    }

    #[test]
    fn slug_empty_string_returns_error() {
        assert!(project_name_to_slack_slug("").is_err());
    }

    #[test]
    fn slug_all_special_chars_returns_error() {
        assert!(project_name_to_slack_slug("!!!???###").is_err());
    }

    #[test]
    fn slug_unicode_chars_become_dashes() {
        // Non-ASCII characters (é, ü) are not ASCII-alphanumeric, so they
        // become dashes, which are then collapsed.
        // "Café München" → lowercase → "café münchen"
        // → ASCII-only: "caf- m-nchen" → collapse → "caf-m-nchen"
        // → prefix → "prj-caf-m-nchen"
        let slug = project_name_to_slack_slug("Café München").unwrap();
        assert_eq!(slug, "prj-caf-m-nchen");
    }
}
