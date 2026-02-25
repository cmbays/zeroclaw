use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::warn;

// ---------------------------------------------------------------------------
// Manifest schema
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    repos: Vec<RepoSpec>,
    global: Option<GlobalSpec>,
    mattermost: Option<MattermostManifestSpec>,
}

/// Optional [mattermost] block in the manifest — all fields can be overridden
/// by CLI flags or environment variables.
#[derive(Debug, Deserialize)]
struct MattermostManifestSpec {
    url: String,
    token: Option<String>,
    team: String,
}

/// A repository group: channels prefixed with `prefix`, bots assigned by role.
#[derive(Debug, Deserialize)]
struct RepoSpec {
    /// Short prefix applied to every channel name, e.g. `"zc"` → `zc-general`.
    prefix: String,
    /// Human-readable project name (used for sidebar category label).
    name: String,
    /// Channel names within this repo (without prefix).
    #[serde(default)]
    channels: Vec<String>,
    /// Bot assignment map. Special key `"all"` → added to every channel.
    /// Other keys are channel names → bots added only to that channel.
    #[serde(default)]
    bots: HashMap<String, Vec<String>>,
    /// Channel names (without prefix) that should receive an incoming webhook.
    #[serde(default)]
    webhooks: Vec<String>,
}

/// Workspace-wide channels visible to all team members.
#[derive(Debug, Deserialize)]
struct GlobalSpec {
    #[serde(default)]
    channels: Vec<String>,
    /// Bot usernames added to every global channel.
    #[serde(default)]
    bots: Vec<String>,
    /// Channel names that should receive an incoming webhook.
    #[serde(default)]
    webhooks: Vec<String>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn handle_channels(
    manifest_path: String,
    url: Option<String>,
    token: Option<String>,
    team: Option<String>,
    dry_run: bool,
) -> Result<()> {
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read manifest: {manifest_path}"))?;
    let manifest: Manifest = toml::from_str(&raw).context("failed to parse manifest TOML")?;

    let base_url = url
        .or_else(|| manifest.mattermost.as_ref().map(|m| m.url.clone()))
        .or_else(|| std::env::var("MM_URL").ok())
        .context(
            "Mattermost URL required: --url flag, MM_URL env var, or [mattermost] url in manifest",
        )?;
    // In dry-run mode the token is optional — no API calls will be made.
    let token = token
        .or_else(|| manifest.mattermost.as_ref().and_then(|m| m.token.clone()))
        .or_else(|| std::env::var("MM_ADMIN_TOKEN").ok())
        .unwrap_or_else(|| {
            if dry_run {
                "(dry-run)".to_string()
            } else {
                String::new()
            }
        });
    if !dry_run && token.is_empty() {
        anyhow::bail!(
            "Admin token required: --token flag, MM_ADMIN_TOKEN env var, or [mattermost] token in manifest"
        );
    }
    let team_name = team
        .or_else(|| manifest.mattermost.as_ref().map(|m| m.team.clone()))
        .or_else(|| std::env::var("MM_TEAM").ok())
        .context(
            "Team name required: --team flag, MM_TEAM env var, or [mattermost] team in manifest",
        )?;

    let base_url = base_url.trim_end_matches('/').to_string();
    let client = Client::new();

    if dry_run {
        println!("[dry-run] No API calls will be made.\n");
    }

    // In dry-run mode skip the network round-trips and use placeholder IDs.
    let (team_id, admin_user_id) = if dry_run {
        println!("Team: {team_name} (dry-run)");
        ("dry-run-team-id".to_string(), "dry-run-user-id".to_string())
    } else {
        let tid = api_get_team_id(&client, &base_url, &token, &team_name).await?;
        println!("Team: {team_name} (id: {tid})");
        let uid = api_get_my_user_id(&client, &base_url, &token).await?;
        (tid, uid)
    };

    // username → user_id cache to avoid duplicate lookups
    let mut user_cache: HashMap<String, String> = HashMap::new();
    // category label → channel IDs, collected during provisioning for sidebar setup
    let mut category_channels: HashMap<String, Vec<String>> = HashMap::new();

    // Global channels
    if let Some(global) = &manifest.global {
        println!("\n[global]");
        for ch_name in &global.channels {
            let channel_id =
                ensure_channel(&client, &base_url, &token, &team_id, ch_name, dry_run).await?;
            add_members(
                &client,
                &base_url,
                &token,
                &channel_id,
                &global.bots,
                &mut user_cache,
                dry_run,
            )
            .await?;
            if global.webhooks.contains(ch_name) {
                create_incoming_webhook(&client, &base_url, &token, &channel_id, ch_name, dry_run)
                    .await?;
            }
            category_channels
                .entry("Global".to_string())
                .or_default()
                .push(channel_id);
        }
    }

    // Repo channels
    for repo in &manifest.repos {
        println!("\n[{}] prefix: {}", repo.name, repo.prefix);
        let category_name = format!("{} ({})", repo.name, repo.prefix);

        for ch_name in &repo.channels {
            let full_name = format!("{}-{}", repo.prefix, ch_name);

            // Collect bots: "all" key + channel-specific key, deduped
            let mut bots: Vec<String> = repo.bots.get("all").cloned().unwrap_or_default();
            if let Some(extra) = repo.bots.get(ch_name.as_str()) {
                for b in extra {
                    if !bots.contains(b) {
                        bots.push(b.clone());
                    }
                }
            }

            let channel_id =
                ensure_channel(&client, &base_url, &token, &team_id, &full_name, dry_run).await?;
            add_members(
                &client,
                &base_url,
                &token,
                &channel_id,
                &bots,
                &mut user_cache,
                dry_run,
            )
            .await?;
            if repo.webhooks.contains(ch_name) {
                create_incoming_webhook(
                    &client,
                    &base_url,
                    &token,
                    &channel_id,
                    &full_name,
                    dry_run,
                )
                .await?;
            }
            category_channels
                .entry(category_name.clone())
                .or_default()
                .push(channel_id);
        }
    }

    // Sidebar categories for the admin user
    if !dry_run && !category_channels.is_empty() {
        println!("\n[sidebar categories]");
        provision_sidebar_categories(
            &client,
            &base_url,
            &token,
            &admin_user_id,
            &team_id,
            &category_channels,
        )
        .await?;
    }

    println!("\nDone.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Mattermost API helpers
// ---------------------------------------------------------------------------

/// Extract a safe error message from a Mattermost API error response.
/// Only exposes the `message` field — avoids dumping full bodies that may
/// contain request metadata.
fn mm_error_msg(body: &Value) -> &str {
    body.get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("(no message)")
}

async fn api_get_team_id(
    client: &Client,
    base_url: &str,
    token: &str,
    team_name: &str,
) -> Result<String> {
    let resp = client
        .get(format!("{base_url}/api/v4/teams/name/{team_name}"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /teams/name/{team_name}")?;
    let status = resp.status();
    let body: Value = resp.json().await.context("parse team response")?;
    if !status.is_success() {
        let msg = mm_error_msg(&body);
        bail!("team '{team_name}' not found: {status} — {msg}");
    }
    body.get("id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .context("team response missing 'id'")
}

async fn api_get_my_user_id(client: &Client, base_url: &str, token: &str) -> Result<String> {
    let resp = client
        .get(format!("{base_url}/api/v4/users/me"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /users/me")?;
    let body: Value = resp.json().await.context("parse /users/me")?;
    body.get("id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .context("/users/me missing 'id'")
}

/// Create the channel if it does not exist; return the channel ID either way.
async fn ensure_channel(
    client: &Client,
    base_url: &str,
    token: &str,
    team_id: &str,
    channel_name: &str,
    dry_run: bool,
) -> Result<String> {
    if dry_run {
        println!("  #{channel_name}  (dry-run)");
        return Ok(format!("dry-run-id-{channel_name}"));
    }

    let check = client
        .get(format!(
            "{base_url}/api/v4/teams/{team_id}/channels/name/{channel_name}"
        ))
        .bearer_auth(token)
        .send()
        .await
        .context("GET channel by name")?;

    if check.status().is_success() {
        let body: Value = check.json().await.context("parse channel response")?;
        let id = body
            .get("id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .context("channel response missing 'id'")?;
        println!("  #{channel_name}  already exists (id: {id})");
        return Ok(id);
    }

    println!("  #{channel_name}  creating...");

    let resp = client
        .post(format!("{base_url}/api/v4/channels"))
        .bearer_auth(token)
        .json(&json!({
            "team_id": team_id,
            "name": channel_name,
            "display_name": channel_name,
            "type": "O",
        }))
        .send()
        .await
        .context("POST /channels")?;
    let status = resp.status();
    let body: Value = resp.json().await.context("parse create channel response")?;
    if !status.is_success() {
        let msg = mm_error_msg(&body);
        bail!("failed to create #{channel_name}: {status} — {msg}");
    }
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .context("create channel response missing 'id'")?;
    println!("  #{channel_name}  created (id: {id})");
    Ok(id)
}

/// Resolve a Mattermost username to a user ID, using `cache` to skip repeat lookups.
async fn resolve_user(
    client: &Client,
    base_url: &str,
    token: &str,
    username: &str,
    cache: &mut HashMap<String, String>,
) -> Result<String> {
    if let Some(id) = cache.get(username) {
        return Ok(id.clone());
    }
    let resp = client
        .get(format!("{base_url}/api/v4/users/username/{username}"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /users/username/{username}")?;
    let status = resp.status();
    let body: Value = resp.json().await.context("parse user response")?;
    if !status.is_success() {
        let msg = mm_error_msg(&body);
        bail!("user '{username}' not found: {status} — {msg}");
    }
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(String::from)
        .context("user response missing 'id'")?;
    cache.insert(username.to_string(), id.clone());
    Ok(id)
}

/// Add a list of bots to a channel. Missing users are warned and skipped; duplicate
/// membership (Mattermost returns 400 with "already") is treated as success.
async fn add_members(
    client: &Client,
    base_url: &str,
    token: &str,
    channel_id: &str,
    bots: &[String],
    user_cache: &mut HashMap<String, String>,
    dry_run: bool,
) -> Result<()> {
    for bot in bots {
        if dry_run {
            println!("    + @{bot}  (dry-run)");
            continue;
        }
        let user_id = match resolve_user(client, base_url, token, bot, user_cache).await {
            Ok(id) => id,
            Err(e) => {
                warn!("skipping @{bot}: {e}");
                continue;
            }
        };
        let resp = client
            .post(format!("{base_url}/api/v4/channels/{channel_id}/members"))
            .bearer_auth(token)
            .json(&json!({ "user_id": user_id }))
            .send()
            .await
            .context("POST /channels/{id}/members")?;
        let status = resp.status();
        if status.is_success() {
            println!("    + @{bot}  added");
        } else {
            let body: Value = resp.json().await.unwrap_or_default();
            // Mattermost returns error id "api.channel.add_member.exists.app_error"
            // when the user is already a channel member.
            let err_id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if err_id.contains("exists") {
                println!("    + @{bot}  already a member");
            } else {
                let msg = mm_error_msg(&body);
                bail!("failed to add @{bot}: {status} — {msg}");
            }
        }
    }
    Ok(())
}

/// Create an incoming webhook for the channel. Prints the resulting hook URL.
/// Non-fatal: logs a warning instead of erroring on failure (webhook creation
/// may require system console config to be enabled).
async fn create_incoming_webhook(
    client: &Client,
    base_url: &str,
    token: &str,
    channel_id: &str,
    channel_name: &str,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        println!("    webhook #{channel_name}: (dry-run)");
        return Ok(());
    }
    let resp = client
        .post(format!("{base_url}/api/v4/hooks/incoming"))
        .bearer_auth(token)
        .json(&json!({
            "channel_id": channel_id,
            "display_name": format!("#{channel_name}"),
            "description": format!("Incoming webhook for #{channel_name}"),
        }))
        .send()
        .await
        .context("POST /hooks/incoming")?;
    let status = resp.status();
    let body: Value = resp.json().await.context("parse webhook response")?;
    if !status.is_success() {
        let msg = mm_error_msg(&body);
        warn!("could not create webhook for #{channel_name}: {status} — {msg}");
        return Ok(());
    }
    // Mattermost webhook URLs use the hook's `id` as the path component.
    let hook_id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("(unknown)");
    // The hook URL grants write access to the channel — store it securely, do not commit.
    println!("    webhook #{channel_name} [SECRET URL]: {base_url}/hooks/{hook_id}");
    Ok(())
}

/// Ensure sidebar categories exist for `user_id` with the given channel groupings.
/// Creates a new custom category if absent; merges channel IDs into an existing one.
/// Non-fatal: warns on API errors rather than failing the entire run.
async fn provision_sidebar_categories(
    client: &Client,
    base_url: &str,
    token: &str,
    user_id: &str,
    team_id: &str,
    category_channels: &HashMap<String, Vec<String>>,
) -> Result<()> {
    let resp = client
        .get(format!(
            "{base_url}/api/v4/users/{user_id}/teams/{team_id}/channels/categories"
        ))
        .bearer_auth(token)
        .send()
        .await
        .context("GET sidebar categories")?;
    if !resp.status().is_success() {
        warn!(
            "cannot read sidebar categories ({}); skipping",
            resp.status()
        );
        return Ok(());
    }
    let existing: Value = resp.json().await.context("parse sidebar categories")?;
    let cats = existing
        .get("categories")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for (display_name, channel_ids) in category_channels {
        let existing_cat = cats.iter().find(|c| {
            c.get("display_name")
                .and_then(|v| v.as_str())
                .map_or(false, |n| n.eq_ignore_ascii_case(display_name))
        });

        if let Some(cat) = existing_cat {
            let cat_id = cat.get("id").and_then(|v| v.as_str()).unwrap_or("");
            // Merge new channel IDs into existing ones
            let mut ids: Vec<String> = cat
                .get("channel_ids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            for cid in channel_ids {
                if !ids.contains(cid) {
                    ids.push(cid.clone());
                }
            }
            let mut updated = cat.clone();
            updated["channel_ids"] =
                Value::Array(ids.iter().map(|s| Value::String(s.clone())).collect());
            let put = client
                .put(format!(
                    "{base_url}/api/v4/users/{user_id}/teams/{team_id}/channels/categories/{cat_id}"
                ))
                .bearer_auth(token)
                .json(&updated)
                .send()
                .await
                .context("PUT sidebar category")?;
            if put.status().is_success() {
                println!("  sidebar '{display_name}'  updated");
            } else {
                warn!("could not update sidebar '{display_name}'");
            }
        } else {
            let post = client
                .post(format!(
                    "{base_url}/api/v4/users/{user_id}/teams/{team_id}/channels/categories"
                ))
                .bearer_auth(token)
                .json(&json!({
                    "user_id": user_id,
                    "team_id": team_id,
                    "type": "custom",
                    "display_name": display_name,
                    "channel_ids": channel_ids,
                }))
                .send()
                .await
                .context("POST sidebar category")?;
            if post.status().is_success() {
                println!("  sidebar '{display_name}'  created");
            } else {
                warn!("could not create sidebar '{display_name}'");
            }
        }
    }
    Ok(())
}
