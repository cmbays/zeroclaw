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
pub fn project_name_to_slack_slug(name: &str) -> String {
    let raw: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse consecutive dashes, strip leading/trailing dashes.
    let collapsed = raw
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    let slug = format!("prj-{collapsed}");
    slug.chars().take(80).collect()
}

/// Scan a Slack channel topic/description for an embedded Linear project URL.
///
/// Returns the URL if found, `None` otherwise.
pub fn detect_channel_project_link(text: &str) -> Option<String> {
    for word in text.split_whitespace() {
        if word.starts_with("https://linear.app/") && word.contains("/project/") {
            return Some(
                word.trim_end_matches(|c: char| !c.is_alphanumeric())
                    .to_string(),
            );
        }
    }
    None
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
    let channel_name = project_name_to_slack_slug(project_name);

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
            project_name_to_slack_slug("Auth Refactor"),
            "prj-auth-refactor"
        );
    }

    #[test]
    fn slug_special_chars() {
        assert_eq!(
            project_name_to_slack_slug("Q1/2026 -- Infra"),
            "prj-q1-2026-infra"
        );
    }

    #[test]
    fn slug_already_lowercase() {
        assert_eq!(project_name_to_slack_slug("billing"), "prj-billing");
    }

    #[test]
    fn slug_truncates_at_80_chars() {
        let long_name = "a".repeat(90);
        let slug = project_name_to_slack_slug(&long_name);
        assert!(slug.len() <= 80, "slug must not exceed 80 chars");
    }

    #[test]
    fn detect_link_present() {
        let text = "Linear project: https://linear.app/acme/project/auth-refactor/ABC123";
        assert!(detect_channel_project_link(text).is_some());
    }

    #[test]
    fn detect_link_absent() {
        assert!(detect_channel_project_link("no links here").is_none());
    }

    #[test]
    fn detect_link_non_project_linear_url() {
        let text = "See https://linear.app/acme/issue/ENG-123";
        assert!(detect_channel_project_link(text).is_none());
    }
}
