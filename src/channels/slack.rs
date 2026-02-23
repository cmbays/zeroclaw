use super::traits::{Channel, ChannelMessage, SendMessage};
use crate::wake_sleep::{EventDecision, WakeSleepEngine, INACTIVITY_TIMEOUT};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::AbortHandle;
use tokio_tungstenite::tungstenite::Message as WsMsg;

/// Socket Mode envelope received over the WebSocket connection.
#[derive(Debug, Deserialize)]
struct SocketModeEnvelope {
    envelope_id: String,
    #[serde(rename = "type")]
    envelope_type: String,
    payload: serde_json::Value,
}

/// Slack channel — Socket Mode WebSocket (primary) or HTTP polling (fallback).
pub struct SlackChannel {
    bot_token: String,
    app_token: Option<String>,
    channel_id: Option<String>,
    allowed_users: Vec<String>,
    wake_sleep: Arc<WakeSleepEngine>,
    timers: Arc<Mutex<HashMap<String, AbortHandle>>>,
}

// ── Capacity limits ──────────────────────────────────────────────────────────

/// Maximum concurrent inactivity timers (one per active thread).
const MAX_ACTIVE_TIMERS: usize = 10_000;

/// Maximum pages fetched from conversations.list (50 × 200 = 10,000 channels).
const MAX_PAGES: usize = 50;

// ── Block Kit field identifiers ─────────────────────────────────────────────

const BK_BLOCK_ISSUE_ACTIONS: &str = "issue_actions";
const BK_ACTION_CONFIRM: &str = "confirm_issue";
const BK_ACTION_EDIT: &str = "edit_issue";
const BK_ACTION_CANCEL: &str = "cancel_issue";
const BK_CALLBACK_EDIT_MODAL: &str = "edit_issue_modal";
const BK_BLOCK_TITLE: &str = "title_block";
const BK_BLOCK_DESCRIPTION: &str = "description_block";
const BK_INPUT_TITLE: &str = "title_input";
const BK_INPUT_DESCRIPTION: &str = "description_input";

impl SlackChannel {
    pub fn new(
        bot_token: String,
        app_token: Option<String>,
        channel_id: Option<String>,
        allowed_users: Vec<String>,
    ) -> Self {
        Self {
            bot_token,
            app_token,
            channel_id,
            allowed_users,
            wake_sleep: Arc::new(WakeSleepEngine::new()),
            timers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn http_client(&self) -> reqwest::Client {
        crate::config::build_runtime_proxy_client("channel.slack")
    }

    /// Check if a Slack user ID is in the allowlist.
    /// Empty list means deny everyone until explicitly configured.
    /// `"*"` means allow everyone.
    fn is_user_allowed(&self, user_id: &str) -> bool {
        self.allowed_users.iter().any(|u| u == "*" || u == user_id)
    }

    /// Get the bot's own user ID so we can ignore our own messages
    async fn get_bot_user_id(&self) -> Option<String> {
        let response = match self
            .http_client()
            .get("https://slack.com/api/auth.test")
            .bearer_auth(&self.bot_token)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Slack: auth.test request failed: {e}");
                return None;
            }
        };

        let resp: serde_json::Value = match response.json().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Slack: auth.test response is not valid JSON: {e}");
                return None;
            }
        };

        resp.get("user_id")
            .and_then(|u| u.as_str())
            .map(String::from)
    }

    /// Resolve the thread identifier for inbound Slack messages.
    /// Replies carry `thread_ts` (root thread id); top-level messages only have `ts`.
    fn inbound_thread_ts(msg: &serde_json::Value, ts: &str) -> Option<String> {
        msg.get("thread_ts")
            .and_then(|t| t.as_str())
            .or(if ts.is_empty() { None } else { Some(ts) })
            .map(str::to_string)
    }

    fn normalized_channel_id(input: Option<&str>) -> Option<String> {
        input
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "*")
            .map(ToOwned::to_owned)
    }

    fn configured_channel_id(&self) -> Option<String> {
        Self::normalized_channel_id(self.channel_id.as_deref())
    }

    fn extract_channel_ids(list_payload: &serde_json::Value) -> Vec<String> {
        let mut ids = list_payload
            .get("channels")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
            .filter_map(|channel| {
                let id = channel.get("id").and_then(|id| id.as_str())?;
                let is_archived = channel
                    .get("is_archived")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let is_member = channel
                    .get("is_member")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if is_archived || !is_member {
                    return None;
                }
                Some(id.to_string())
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids.dedup();
        ids
    }

    async fn list_accessible_channels(&self) -> anyhow::Result<Vec<String>> {
        let mut channels = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages: usize = 0;

        loop {
            pages += 1;
            let mut query_params = vec![
                ("exclude_archived", "true".to_string()),
                ("limit", "200".to_string()),
                (
                    "types",
                    "public_channel,private_channel,mpim,im".to_string(),
                ),
            ];
            if let Some(ref next) = cursor {
                query_params.push(("cursor", next.clone()));
            }

            let resp = self
                .http_client()
                .get("https://slack.com/api/conversations.list")
                .bearer_auth(&self.bot_token)
                .query(&query_params)
                .send()
                .await?;

            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));

            if !status.is_success() {
                anyhow::bail!("Slack conversations.list failed ({status}): {body}");
            }

            let data: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
                anyhow::anyhow!("Slack conversations.list: response is not valid JSON: {e}")
            })?;
            if data.get("ok") == Some(&serde_json::Value::Bool(false)) {
                let err = data
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("unknown");
                anyhow::bail!("Slack conversations.list failed: {err}");
            }

            channels.extend(Self::extract_channel_ids(&data));

            cursor = data
                .get("response_metadata")
                .and_then(|rm| rm.get("next_cursor"))
                .and_then(|c| c.as_str())
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(ToOwned::to_owned);

            if cursor.is_none() {
                break;
            }
            if pages >= MAX_PAGES {
                tracing::warn!(
                    pages = MAX_PAGES,
                    "Slack: conversations.list reached page limit; channel list may be incomplete"
                );
                break;
            }
        }

        channels.sort();
        channels.dedup();
        Ok(channels)
    }

    // ── Socket Mode helpers ──────────────────────────────────────

    /// Call `apps.connections.open` to obtain a WebSocket URL.
    async fn open_socket_mode_connection(&self) -> anyhow::Result<String> {
        let app_token = self
            .app_token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Slack: app_token required for Socket Mode"))?;

        let resp: serde_json::Value = self
            .http_client()
            .post("https://slack.com/api/apps.connections.open")
            .bearer_auth(app_token)
            .send()
            .await?
            .json()
            .await?;

        if resp.get("ok") != Some(&serde_json::Value::Bool(true)) {
            let err = resp
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("Slack apps.connections.open failed: {err}");
        }

        let url = resp
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| anyhow::anyhow!("Slack: apps.connections.open returned no URL"))?;

        Self::validate_wss_url(url)?;
        Ok(url.to_string())
    }

    /// Validate that a Socket Mode WebSocket URL uses wss:// on a Slack domain.
    fn validate_wss_url(url: &str) -> anyhow::Result<()> {
        if !url.starts_with("wss://") {
            anyhow::bail!(
                "Slack: WebSocket URL must use wss:// scheme, got: {}",
                url.split("://").next().unwrap_or("unknown")
            );
        }
        let host = url
            .strip_prefix("wss://")
            .and_then(|rest| rest.split('/').next())
            .and_then(|host_port| host_port.split(':').next())
            .unwrap_or("");
        if host != "slack.com" && !host.ends_with(".slack.com") {
            anyhow::bail!("Slack: WebSocket URL host must be *.slack.com, got: {host}");
        }
        Ok(())
    }

    /// Parse a Socket Mode JSON envelope from a string.
    ///
    /// Production code uses `serde_json::from_value` on an already-parsed `Value`
    /// to avoid double-parsing. This function is kept for tests.
    #[cfg(test)]
    fn parse_envelope(text: &str) -> anyhow::Result<SocketModeEnvelope> {
        serde_json::from_str(text).map_err(|e| anyhow::anyhow!("Slack: envelope parse error: {e}"))
    }

    /// Format a Slack message ID from channel and timestamp.
    fn message_id(channel_id: &str, ts: &str) -> String {
        format!("slack_{channel_id}_{ts}")
    }

    /// Current Unix timestamp in whole seconds.
    fn unix_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Extract message fields from an `events_api` payload.
    /// Returns `(user, text, channel, ts, thread_ts)` or `None` if the event should be skipped.
    fn extract_event_message(
        payload: &serde_json::Value,
        bot_user_id: &str,
    ) -> Option<(String, String, String, String, Option<String>)> {
        let event = payload.get("event")?;
        let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

        // Only process message and app_mention events
        if event_type != "message" && event_type != "app_mention" {
            return None;
        }

        // Skip message subtypes (message_changed, message_deleted, bot_message, etc.)
        if event.get("subtype").is_some() {
            return None;
        }

        // Skip bot messages (have bot_id field)
        if event.get("bot_id").is_some() {
            return None;
        }

        let user = event.get("user").and_then(|u| u.as_str()).unwrap_or("");
        let text = event.get("text").and_then(|t| t.as_str()).unwrap_or("");
        let channel = event.get("channel").and_then(|c| c.as_str()).unwrap_or("");
        let ts = event.get("ts").and_then(|t| t.as_str()).unwrap_or("");

        // Skip bot's own messages
        if user == bot_user_id {
            return None;
        }

        // Skip empty messages
        if text.is_empty() || user.is_empty() || channel.is_empty() || ts.is_empty() {
            return None;
        }

        // ts is guaranteed non-empty at this point
        let thread_ts = event
            .get("thread_ts")
            .and_then(|t| t.as_str())
            .or(Some(ts))
            .map(str::to_string);

        Some((
            user.to_string(),
            text.to_string(),
            channel.to_string(),
            ts.to_string(),
            thread_ts,
        ))
    }

    /// Reset the inactivity timer for a thread.
    ///
    /// Aborts any existing timer for this thread then spawns a new one that
    /// will call `mark_sleeping` and post a sleep notification after
    /// [`INACTIVITY_TIMEOUT`].
    fn reset_inactivity_timer(&self, thread_key: &str, channel: &str, thread_ts: &str) {
        // Abort previous timer for this thread, if any.
        if let Some(h) = self
            .timers
            .lock()
            .expect("timers mutex poisoned")
            .remove(thread_key)
        {
            h.abort();
        }

        let wake_sleep: Arc<WakeSleepEngine> = Arc::clone(&self.wake_sleep);
        let thread_key_owned = thread_key.to_string();
        let bot_token = self.bot_token.clone();
        let http_client = self.http_client();
        let channel_owned = channel.to_string();
        let thread_ts_owned = thread_ts.to_string();

        let handle = tokio::spawn(async move {
            tokio::time::sleep(INACTIVITY_TIMEOUT).await;

            wake_sleep.mark_sleeping(&thread_key_owned);
            tracing::info!("Slack: thread {thread_key_owned} went to sleep (inactivity)");

            // Post sleep notification.
            let body = serde_json::json!({
                "channel": channel_owned,
                "thread_ts": thread_ts_owned,
                "text": "Going to sleep — @mention me to wake up :zzz:"
            });
            match http_client
                .post("https://slack.com/api/chat.postMessage")
                .bearer_auth(&bot_token)
                .json(&body)
                .send()
                .await
            {
                Err(e) => {
                    tracing::warn!("Slack: failed to post sleep notification: {e}");
                }
                Ok(resp) => match resp.json::<serde_json::Value>().await {
                    Ok(data) if data.get("ok") != Some(&serde_json::Value::Bool(true)) => {
                        let err = data
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("unknown");
                        tracing::warn!("Slack: sleep notification rejected by API: {err}");
                    }
                    Err(e) => {
                        tracing::warn!("Slack: failed to parse sleep notification response: {e}");
                    }
                    Ok(_) => {}
                },
            }
        })
        .abort_handle();

        let mut guard = self.timers.lock().expect("timers mutex poisoned");
        if guard.len() >= MAX_ACTIVE_TIMERS {
            tracing::warn!(
                capacity = MAX_ACTIVE_TIMERS,
                "Slack: inactivity timer capacity reached; aborting timer for {thread_key}"
            );
            handle.abort();
        } else {
            guard.insert(thread_key.to_string(), handle);
        }
    }

    // ── Interactive flow helpers ──────────────────────────────────────

    /// Escape Slack mrkdwn special characters in user-supplied strings.
    ///
    /// Prevents `<@U123>` @-mentions, injected links (`<url|text>`), and
    /// formatting breakage from user-controlled content.
    fn escape_mrkdwn(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// Build Block Kit blocks for an issue draft message.
    ///
    /// Renders a summary section followed by Confirm / Edit / Cancel action buttons.
    /// `title` is carried in each button's value so handlers can recover context.
    pub fn build_issue_draft_blocks(title: &str, description: &str) -> serde_json::Value {
        let title_safe = Self::escape_mrkdwn(title);
        let description_safe = Self::escape_mrkdwn(description);
        let summary =
            format!("*Draft Issue*\n*Title:* {title_safe}\n*Description:* {description_safe}");
        serde_json::json!([
            {
                "type": "section",
                "text": {"type": "mrkdwn", "text": summary}
            },
            {
                "type": "actions",
                "block_id": BK_BLOCK_ISSUE_ACTIONS,
                "elements": [
                    {
                        "type": "button",
                        "text": {"type": "plain_text", "text": "Confirm"},
                        "action_id": BK_ACTION_CONFIRM,
                        "value": title,
                        "style": "primary"
                    },
                    {
                        "type": "button",
                        "text": {"type": "plain_text", "text": "Edit"},
                        "action_id": BK_ACTION_EDIT,
                        "value": title
                    },
                    {
                        "type": "button",
                        "text": {"type": "plain_text", "text": "Cancel"},
                        "action_id": BK_ACTION_CANCEL,
                        "style": "danger"
                    }
                ]
            }
        ])
    }

    /// Build a Block Kit modal view for editing an issue draft.
    ///
    /// `private_metadata` carries `"<channel_id>:<thread_ts>"` so the view
    /// submission handler can reconstruct the reply context.
    fn build_issue_modal(initial_title: &str, private_metadata: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "modal",
            "callback_id": BK_CALLBACK_EDIT_MODAL,
            "private_metadata": private_metadata,
            "title": {"type": "plain_text", "text": "Edit Issue"},
            "submit": {"type": "plain_text", "text": "Submit"},
            "close": {"type": "plain_text", "text": "Cancel"},
            "blocks": [
                {
                    "type": "input",
                    "block_id": BK_BLOCK_TITLE,
                    "label": {"type": "plain_text", "text": "Title"},
                    "element": {
                        "type": "plain_text_input",
                        "action_id": BK_INPUT_TITLE,
                        "initial_value": initial_title
                    }
                },
                {
                    "type": "input",
                    "block_id": BK_BLOCK_DESCRIPTION,
                    "label": {"type": "plain_text", "text": "Description"},
                    "element": {
                        "type": "plain_text_input",
                        "action_id": BK_INPUT_DESCRIPTION,
                        "multiline": true,
                        "initial_value": ""
                    }
                }
            ]
        })
    }

    /// Build Block Kit blocks for an issue confirmation message.
    ///
    /// Renders a checkmark followed by a link to the created Linear issue.
    pub fn build_issue_confirmation_blocks(title: &str, url: &str) -> serde_json::Value {
        let title_safe = Self::escape_mrkdwn(title);
        // Percent-encode `|` in the URL to prevent display-text injection in
        // Slack's mrkdwn link format `<url|text>`. Do not apply escape_mrkdwn to
        // the URL — HTML entity-encoding `&` to `&amp;` would corrupt query strings.
        let url_safe = url.replace('|', "%7C");
        let text = format!(":white_check_mark: *Issue created:* <{url_safe}|{title_safe}>");
        serde_json::json!([
            {
                "type": "section",
                "text": {"type": "mrkdwn", "text": text}
            }
        ])
    }

    /// Extract required context from a `block_actions` interactive payload.
    ///
    /// Returns `(user_id, channel_id, thread_ts, action_id, value, trigger_id)`,
    /// or `None` if a required field is absent.
    fn parse_block_action_context(
        payload: &serde_json::Value,
    ) -> Option<(String, String, Option<String>, String, String, String)> {
        let user = payload
            .get("user")
            .and_then(|u| u.get("id"))
            .and_then(|id| id.as_str())
            .filter(|s| !s.is_empty())?
            .to_string();

        let channel = payload
            .get("channel")
            .and_then(|c| c.get("id"))
            .and_then(|id| id.as_str())
            .filter(|s| !s.is_empty())?
            .to_string();

        let thread_ts = payload
            .get("message")
            .and_then(|m| m.get("thread_ts").or_else(|| m.get("ts")))
            .and_then(|ts| ts.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let action = payload
            .get("actions")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())?;

        let action_id = action
            .get("action_id")
            .and_then(|id| id.as_str())
            .filter(|s| !s.is_empty())?
            .to_string();

        let value = action
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let trigger_id = payload
            .get("trigger_id")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        Some((user, channel, thread_ts, action_id, value, trigger_id))
    }

    /// Handle a `block_actions` interactive payload.
    ///
    /// `edit_issue` actions open a modal via [`open_modal`]; all other actions
    /// are forwarded as synthetic [`ChannelMessage`]s to the agent.
    async fn handle_block_action(
        &self,
        payload: &serde_json::Value,
        scoped_channel: Option<&str>,
        tx: &tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        let Some((user, channel, thread_ts, action_id, value, trigger_id)) =
            Self::parse_block_action_context(payload)
        else {
            tracing::warn!("Slack: block_action payload missing required fields");
            return Ok(());
        };

        if !self.is_user_allowed(&user) {
            tracing::debug!("Slack: ignoring block_action from unauthorized user: {user}");
            return Ok(());
        }

        if let Some(scoped) = scoped_channel {
            if channel != scoped {
                return Ok(());
            }
        }

        if action_id == BK_ACTION_EDIT {
            if trigger_id.is_empty() {
                tracing::warn!("Slack: edit_issue action missing trigger_id — cannot open modal");
                return Ok(());
            }
            let meta = match &thread_ts {
                Some(ts) => format!("{channel}:{ts}"),
                None => channel.clone(),
            };
            if let Err(e) = self.open_modal(&trigger_id, &value, &meta).await {
                tracing::warn!("Slack: views.open failed: {e}");
            }
            return Ok(());
        }

        if !matches!(action_id.as_str(), BK_ACTION_CONFIRM | BK_ACTION_CANCEL) {
            tracing::debug!("Slack: block_action unknown action_id: {action_id}");
            return Ok(());
        }

        let safe_value = value.replace(['[', ']'], "");
        let content = format!("[block_action:{action_id}] {safe_value}");
        let id_suffix = thread_ts.as_deref().unwrap_or(&channel).to_string();
        let thread_key = format!("{channel}:{id_suffix}");
        let channel_msg = ChannelMessage {
            id: format!("slack_action_{channel}_{id_suffix}"),
            sender: user,
            reply_target: channel.clone(),
            content,
            channel: "slack".to_string(),
            timestamp: Self::unix_timestamp(),
            thread_ts,
        };

        if tx.send(channel_msg).await.is_err() {
            anyhow::bail!("Slack: message channel closed");
        }

        self.reset_inactivity_timer(&thread_key, &channel, &id_suffix);
        Ok(())
    }

    /// Open a Slack modal for issue editing.
    ///
    /// `private_metadata` is passed through to the modal and recovered on view
    /// submission to identify the reply target channel and thread.
    async fn open_modal(
        &self,
        trigger_id: &str,
        initial_title: &str,
        private_metadata: &str,
    ) -> anyhow::Result<()> {
        let view = Self::build_issue_modal(initial_title, private_metadata);
        let body = serde_json::json!({"trigger_id": trigger_id, "view": view});

        let resp = self
            .http_client()
            .post("https://slack.com/api/views.open")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let body_text = resp
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));

        if !status.is_success() {
            anyhow::bail!("Slack views.open failed ({status}): {body_text}");
        }

        let parsed: serde_json::Value = serde_json::from_str(&body_text)
            .map_err(|e| anyhow::anyhow!("Slack views.open: response is not valid JSON: {e}"))?;
        if parsed.get("ok") == Some(&serde_json::Value::Bool(false)) {
            let err = parsed
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("Slack views.open failed: {err}");
        }

        Ok(())
    }

    /// Handle a `view_submission` interactive payload.
    ///
    /// Extracts the modal form values and reply context from `private_metadata`,
    /// then forwards a synthetic [`ChannelMessage`] to the agent.
    async fn handle_view_submission(
        &self,
        payload: &serde_json::Value,
        scoped_channel: Option<&str>,
        tx: &tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        let user = payload
            .get("user")
            .and_then(|u| u.get("id"))
            .and_then(|id| id.as_str())
            .unwrap_or("");

        if user.is_empty() || !self.is_user_allowed(user) {
            tracing::debug!("Slack: ignoring view_submission from unauthorized user");
            return Ok(());
        }

        let view = match payload.get("view") {
            Some(v) => v,
            None => {
                tracing::warn!("Slack: view_submission missing view field");
                return Ok(());
            }
        };

        let callback_id = view
            .get("callback_id")
            .and_then(|id| id.as_str())
            .unwrap_or("unknown");

        // Decode channel and thread_ts from private_metadata ("channel_id:thread_ts")
        let private_metadata = view
            .get("private_metadata")
            .and_then(|m| m.as_str())
            .unwrap_or("");

        let (channel, thread_ts) = if let Some((c, t)) = private_metadata.split_once(':') {
            (c.to_string(), Some(t.to_string()))
        } else {
            (private_metadata.to_string(), None)
        };

        if channel.is_empty() {
            tracing::warn!("Slack: view_submission missing channel in private_metadata");
            return Ok(());
        }

        // Scope filter: reject if submission targets an unscoped channel.
        if let Some(scoped) = scoped_channel {
            if channel != scoped {
                return Ok(());
            }
        }

        let state = view.get("state").and_then(|s| s.get("values"));

        let title = state
            .and_then(|s| s.get(BK_BLOCK_TITLE))
            .and_then(|b| b.get(BK_INPUT_TITLE))
            .and_then(|i| i.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let description = state
            .and_then(|s| s.get(BK_BLOCK_DESCRIPTION))
            .and_then(|b| b.get(BK_INPUT_DESCRIPTION))
            .and_then(|i| i.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let content =
            format!("[view_submission:{callback_id}] title={title} description={description}");

        let id_suffix = thread_ts.as_deref().unwrap_or(&channel).to_string();
        let thread_key = format!("{channel}:{id_suffix}");
        let channel_msg = ChannelMessage {
            id: format!("slack_view_{channel}_{id_suffix}"),
            sender: user.to_string(),
            reply_target: channel.clone(),
            content,
            channel: "slack".to_string(),
            timestamp: Self::unix_timestamp(),
            thread_ts,
        };

        if tx.send(channel_msg).await.is_err() {
            anyhow::bail!("Slack: message channel closed");
        }

        self.reset_inactivity_timer(&thread_key, &channel, &id_suffix);
        Ok(())
    }

    /// Build the JSON body for `chat.postMessage` from a `SendMessage`.
    fn build_send_body(message: &SendMessage) -> serde_json::Value {
        let mut body = serde_json::json!({
            "channel": message.recipient,
            "text": message.content
        });

        if let Some(ref ts) = message.thread_ts {
            body["thread_ts"] = serde_json::json!(ts);
        }
        if let Some(ref username) = message.username {
            body["username"] = serde_json::json!(username);
        }
        if let Some(ref icon_emoji) = message.icon_emoji {
            body["icon_emoji"] = serde_json::json!(icon_emoji);
        }
        if let Some(ref blocks) = message.blocks {
            body["blocks"] = blocks.clone();
        }
        if let Some(true) = message.reply_broadcast {
            body["reply_broadcast"] = serde_json::json!(true);
        }

        body
    }

    /// Socket Mode WebSocket listen loop. Returns on connection loss (caller reconnects).
    #[allow(clippy::too_many_lines)]
    async fn listen_socket_mode(
        &self,
        tx: &tokio::sync::mpsc::Sender<ChannelMessage>,
        bot_user_id: &str,
    ) -> anyhow::Result<()> {
        let wss_url = self.open_socket_mode_connection().await?;
        tracing::info!("Slack: Socket Mode connecting...");

        let (ws_stream, _) = tokio_tungstenite::connect_async(&wss_url).await?;
        let (mut write, mut read) = ws_stream.split();
        tracing::info!("Slack: Socket Mode connected");

        let scoped_channel = self.configured_channel_id();
        let mut timeout_check = tokio::time::interval(Duration::from_secs(30));
        timeout_check.tick().await; // consume immediate tick
        let mut last_recv = Instant::now();

        loop {
            tokio::select! {
                biased;

                _ = timeout_check.tick() => {
                    if last_recv.elapsed() > Duration::from_secs(60) {
                        tracing::warn!("Slack: Socket Mode heartbeat timeout, reconnecting");
                        break;
                    }
                }

                msg = read.next() => {
                    match msg {
                        Some(Ok(WsMsg::Text(text))) => {
                            last_recv = Instant::now();

                            // Pre-parse to handle non-envelope messages (hello, disconnect)
                            let raw: serde_json::Value = match serde_json::from_str(&text) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::warn!("Slack: unparseable message: {e}");
                                    continue;
                                }
                            };

                            let msg_type = raw.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            match msg_type {
                                "hello" => {
                                    tracing::info!("Slack: Socket Mode hello received");
                                    continue;
                                }
                                "disconnect" => {
                                    let reason = raw.get("reason").and_then(|r| r.as_str()).unwrap_or("unknown");
                                    tracing::info!("Slack: disconnect requested (reason: {reason}), reconnecting");
                                    break;
                                }
                                _ => {}
                            }

                            // Deserialize from the already-parsed Value (avoids re-parsing the string).
                            let envelope = match serde_json::from_value::<SocketModeEnvelope>(raw) {
                                Ok(e) => e,
                                Err(e) => {
                                    tracing::warn!("Slack: envelope parse error: {e}");
                                    continue;
                                }
                            };

                            // ACK immediately (must be <3s)
                            let ack = serde_json::json!({"envelope_id": envelope.envelope_id});
                            if write.send(WsMsg::Text(ack.to_string().into())).await.is_err() {
                                tracing::warn!("Slack: ACK send failed, reconnecting");
                                break;
                            }

                            self.dispatch_envelope(envelope, bot_user_id, scoped_channel.as_deref(), tx).await?;
                        }
                        Some(Ok(WsMsg::Ping(d))) => {
                            last_recv = Instant::now();
                            let _ = write.send(WsMsg::Pong(d)).await;
                        }
                        Some(Ok(WsMsg::Close(_))) => {
                            tracing::info!("Slack: Socket Mode closed by server, reconnecting");
                            break;
                        }
                        Some(Err(e)) => {
                            tracing::error!("Slack: Socket Mode read error: {e}");
                            break;
                        }
                        None => {
                            tracing::info!("Slack: Socket Mode stream ended, reconnecting");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }

    /// Dispatch a parsed Socket Mode envelope to the appropriate handler.
    /// Returns `Err` if the message channel closed (caller should exit).
    async fn dispatch_envelope(
        &self,
        envelope: SocketModeEnvelope,
        bot_user_id: &str,
        scoped_channel: Option<&str>,
        tx: &tokio::sync::mpsc::Sender<ChannelMessage>,
    ) -> anyhow::Result<()> {
        match envelope.envelope_type.as_str() {
            "events_api" => {
                let (user, text, channel, ts, thread_ts) =
                    match Self::extract_event_message(&envelope.payload, bot_user_id) {
                        Some(fields) => fields,
                        None => return Ok(()),
                    };

                // Filter by configured channel_id (parity with polling path)
                if let Some(scoped) = scoped_channel {
                    if channel != scoped {
                        return Ok(());
                    }
                }

                if !self.is_user_allowed(&user) {
                    tracing::debug!("Slack: ignoring message from unauthorized user: {user}");
                    return Ok(());
                }

                // Wake/sleep filtering: @mentions wake sleeping threads; other
                // messages in sleeping threads are discarded.
                let event_type = envelope
                    .payload
                    .get("event")
                    .and_then(|e| e.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let is_mention = event_type == "app_mention";
                let thread_key = format!("{}:{}", channel, thread_ts.as_deref().unwrap_or(&ts));

                match self.wake_sleep.on_event(&thread_key, is_mention) {
                    EventDecision::Forward => {}
                    EventDecision::Wake => {
                        tracing::info!("Slack: thread {thread_key} woke up");
                    }
                    EventDecision::Discard => {
                        tracing::debug!("Slack: thread {thread_key} sleeping — discarding event");
                        return Ok(());
                    }
                }

                // Capture timer ts before thread_ts is moved into channel_msg.
                let timer_ts = thread_ts.as_deref().unwrap_or(&ts).to_string();

                let channel_msg = ChannelMessage {
                    id: Self::message_id(&channel, &ts),
                    sender: user,
                    reply_target: channel.clone(),
                    content: text,
                    channel: "slack".to_string(),
                    timestamp: Self::unix_timestamp(),
                    thread_ts,
                };

                if tx.send(channel_msg).await.is_err() {
                    anyhow::bail!("Slack: message channel closed");
                }

                // Reset per-thread inactivity timer.
                self.reset_inactivity_timer(&thread_key, &channel, &timer_ts);
            }
            "interactive" => {
                let payload_type = envelope
                    .payload
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                match payload_type {
                    "block_actions" => {
                        self.handle_block_action(&envelope.payload, scoped_channel, tx)
                            .await?;
                    }
                    "view_submission" => {
                        self.handle_view_submission(&envelope.payload, scoped_channel, tx)
                            .await?;
                    }
                    other => {
                        tracing::debug!("Slack: unknown interactive payload type: {other}");
                    }
                }
            }
            "slash_commands" => {
                tracing::debug!("Slack: slash command (ignored)");
            }
            other => {
                tracing::debug!("Slack: unknown envelope type: {other}");
            }
        }
        Ok(())
    }

    /// HTTP polling listen loop (fallback when `app_token` is not set).
    async fn listen_polling(
        &self,
        tx: tokio::sync::mpsc::Sender<ChannelMessage>,
        bot_user_id: &str,
    ) -> anyhow::Result<()> {
        let scoped_channel = self.configured_channel_id();
        let mut discovered_channels: Vec<String> = Vec::new();
        let mut last_discovery = Instant::now();
        let mut last_ts_by_channel: HashMap<String, String> = HashMap::new();

        if let Some(ref channel_id) = scoped_channel {
            tracing::info!("Slack channel listening on #{channel_id}...");
        } else {
            tracing::info!(
                "Slack channel_id not set (or '*'); listening across all accessible channels."
            );
        }

        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;

            let target_channels = if let Some(ref channel_id) = scoped_channel {
                vec![channel_id.clone()]
            } else {
                if discovered_channels.is_empty()
                    || last_discovery.elapsed() >= Duration::from_secs(60)
                {
                    match self.list_accessible_channels().await {
                        Ok(channels) => {
                            if channels != discovered_channels {
                                tracing::info!(
                                    "Slack auto-discovery refreshed: listening on {} channel(s).",
                                    channels.len()
                                );
                            }
                            discovered_channels = channels;
                        }
                        Err(e) => {
                            tracing::warn!("Slack channel discovery failed: {e}");
                        }
                    }
                    last_discovery = Instant::now();
                }

                discovered_channels.clone()
            };

            if target_channels.is_empty() {
                tracing::debug!("Slack: no accessible channels discovered yet");
                continue;
            }

            for channel_id in target_channels {
                let mut params = vec![("channel", channel_id.clone()), ("limit", "10".to_string())];
                if let Some(last_ts) = last_ts_by_channel.get(&channel_id).cloned() {
                    if !last_ts.is_empty() {
                        params.push(("oldest", last_ts));
                    }
                }

                let resp = match self
                    .http_client()
                    .get("https://slack.com/api/conversations.history")
                    .bearer_auth(&self.bot_token)
                    .query(&params)
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("Slack poll error for channel {channel_id}: {e}");
                        continue;
                    }
                };

                let data: serde_json::Value = match resp.json().await {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!("Slack parse error for channel {channel_id}: {e}");
                        continue;
                    }
                };

                if data.get("ok") == Some(&serde_json::Value::Bool(false)) {
                    let err = data
                        .get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or("unknown");
                    tracing::warn!("Slack history error for channel {channel_id}: {err}");
                    continue;
                }

                if let Some(messages) = data.get("messages").and_then(|m| m.as_array()) {
                    // Messages come newest-first, reverse to process oldest first
                    for msg in messages.iter().rev() {
                        let ts = msg.get("ts").and_then(|t| t.as_str()).unwrap_or("");
                        let user = msg
                            .get("user")
                            .and_then(|u| u.as_str())
                            .unwrap_or("unknown");
                        let text = msg.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        let last_ts = last_ts_by_channel
                            .get(&channel_id)
                            .map(String::as_str)
                            .unwrap_or("");

                        // Skip bot's own messages
                        if user == bot_user_id {
                            continue;
                        }

                        // Sender validation
                        if !self.is_user_allowed(user) {
                            tracing::debug!(
                                "Slack: ignoring message from unauthorized user: {user}"
                            );
                            continue;
                        }

                        // Skip empty or already-seen
                        if text.is_empty() || ts <= last_ts {
                            continue;
                        }

                        last_ts_by_channel.insert(channel_id.clone(), ts.to_string());

                        let channel_msg = ChannelMessage {
                            id: Self::message_id(&channel_id, ts),
                            sender: user.to_string(),
                            reply_target: channel_id.clone(),
                            content: text.to_string(),
                            channel: "slack".to_string(),
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                            thread_ts: Self::inbound_thread_ts(msg, ts),
                        };

                        if tx.send(channel_msg).await.is_err() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        "slack"
    }

    async fn send(&self, message: &SendMessage) -> anyhow::Result<()> {
        let body = Self::build_send_body(message);

        let resp = self
            .http_client()
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .unwrap_or_else(|e| format!("<failed to read response body: {e}>"));

        if !status.is_success() {
            anyhow::bail!("Slack chat.postMessage failed ({status}): {body}");
        }

        // Slack returns 200 for most app-level errors; check JSON "ok" field
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        if parsed.get("ok") == Some(&serde_json::Value::Bool(false)) {
            let err = parsed
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown");
            anyhow::bail!("Slack chat.postMessage failed: {err}");
        }

        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> anyhow::Result<()> {
        if self.allowed_users.is_empty() {
            tracing::warn!(
                "Slack: allowed_users is empty — all messages will be silently ignored. \
                 Add user IDs to [channels.slack].allowed_users or use [\"*\"] to allow everyone."
            );
        }

        if self.app_token.is_some() {
            // Socket Mode: reconnect with exponential backoff
            let mut backoff = Duration::from_secs(1);
            let max_backoff = Duration::from_secs(60);

            loop {
                // Re-fetch bot_user_id on each reconnect to recover from startup failures
                let bot_user_id = match self.get_bot_user_id().await {
                    Some(id) => id,
                    None => {
                        tracing::warn!(
                            "Slack: failed to resolve bot user ID; self-message filtering degraded"
                        );
                        String::new()
                    }
                };

                let connect_time = Instant::now();
                match self.listen_socket_mode(&tx, &bot_user_id).await {
                    Ok(()) => {
                        // Reset backoff if connection was healthy for a meaningful duration
                        if connect_time.elapsed() > Duration::from_secs(30) {
                            backoff = Duration::from_secs(1);
                        }
                    }
                    Err(e) => {
                        tracing::error!("Slack: Socket Mode error: {e}");
                        if connect_time.elapsed() > Duration::from_secs(30) {
                            backoff = Duration::from_secs(1);
                        }
                    }
                }

                tracing::info!("Slack: reconnecting in {}s", backoff.as_secs());
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
            }
        } else {
            // Fallback: HTTP polling (no app_token)
            tracing::info!("Slack: no app_token set, falling back to HTTP polling");
            let bot_user_id = match self.get_bot_user_id().await {
                Some(id) => id,
                None => {
                    tracing::warn!(
                        "Slack: failed to resolve bot user ID; self-message filtering degraded"
                    );
                    String::new()
                }
            };
            self.listen_polling(tx, &bot_user_id).await
        }
    }

    async fn health_check(&self) -> bool {
        self.http_client()
            .get("https://slack.com/api/auth.test")
            .bearer_auth(&self.bot_token)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_channel() -> SlackChannel {
        SlackChannel::new("xoxb-fake".into(), None, None, vec![])
    }

    #[test]
    fn slack_channel_name() {
        let ch = test_channel();
        assert_eq!(ch.name(), "slack");
    }

    #[test]
    fn slack_channel_with_channel_id() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, Some("C12345".into()), vec![]);
        assert_eq!(ch.channel_id, Some("C12345".to_string()));
    }

    #[test]
    fn slack_channel_with_app_token() {
        let ch = SlackChannel::new("xoxb-fake".into(), Some("xapp-fake".into()), None, vec![]);
        assert_eq!(ch.app_token, Some("xapp-fake".to_string()));
    }

    #[test]
    fn normalized_channel_id_respects_wildcard_and_blank() {
        assert_eq!(SlackChannel::normalized_channel_id(None), None);
        assert_eq!(SlackChannel::normalized_channel_id(Some("")), None);
        assert_eq!(SlackChannel::normalized_channel_id(Some("   ")), None);
        assert_eq!(SlackChannel::normalized_channel_id(Some("*")), None);
        assert_eq!(SlackChannel::normalized_channel_id(Some(" * ")), None);
        assert_eq!(
            SlackChannel::normalized_channel_id(Some(" C12345 ")),
            Some("C12345".to_string())
        );
    }

    #[test]
    fn extract_channel_ids_filters_archived_and_non_member_entries() {
        let payload = serde_json::json!({
            "channels": [
                {"id": "C1", "is_archived": false, "is_member": true},
                {"id": "C2", "is_archived": true, "is_member": true},
                {"id": "C3", "is_archived": false, "is_member": false},
                {"id": "C1", "is_archived": false, "is_member": true},
                {"id": "C4"}
            ]
        });
        let ids = SlackChannel::extract_channel_ids(&payload);
        assert_eq!(ids, vec!["C1".to_string(), "C4".to_string()]);
    }

    #[test]
    fn empty_allowlist_denies_everyone() {
        let ch = test_channel();
        assert!(!ch.is_user_allowed("U12345"));
        assert!(!ch.is_user_allowed("anyone"));
    }

    #[test]
    fn wildcard_allows_everyone() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec!["*".into()]);
        assert!(ch.is_user_allowed("U12345"));
    }

    #[test]
    fn specific_allowlist_filters() {
        let ch = SlackChannel::new(
            "xoxb-fake".into(),
            None,
            None,
            vec!["U111".into(), "U222".into()],
        );
        assert!(ch.is_user_allowed("U111"));
        assert!(ch.is_user_allowed("U222"));
        assert!(!ch.is_user_allowed("U333"));
    }

    #[test]
    fn allowlist_exact_match_not_substring() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec!["U111".into()]);
        assert!(!ch.is_user_allowed("U1111"));
        assert!(!ch.is_user_allowed("U11"));
    }

    #[test]
    fn allowlist_empty_user_id() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec!["U111".into()]);
        assert!(!ch.is_user_allowed(""));
    }

    #[test]
    fn allowlist_case_sensitive() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec!["U111".into()]);
        assert!(ch.is_user_allowed("U111"));
        assert!(!ch.is_user_allowed("u111"));
    }

    #[test]
    fn allowlist_wildcard_and_specific() {
        let ch = SlackChannel::new(
            "xoxb-fake".into(),
            None,
            None,
            vec!["U111".into(), "*".into()],
        );
        assert!(ch.is_user_allowed("U111"));
        assert!(ch.is_user_allowed("anyone"));
    }

    // ── Message ID edge cases ─────────────────────────────────────

    #[test]
    fn slack_message_id_format_includes_channel_and_ts() {
        let id = SlackChannel::message_id("C12345", "1234567890.123456");
        assert_eq!(id, "slack_C12345_1234567890.123456");
    }

    #[test]
    fn slack_message_id_is_deterministic() {
        let id1 = SlackChannel::message_id("C12345", "1234567890.123456");
        let id2 = SlackChannel::message_id("C12345", "1234567890.123456");
        assert_eq!(id1, id2);
    }

    #[test]
    fn slack_message_id_different_ts_different_id() {
        let id1 = SlackChannel::message_id("C12345", "1234567890.123456");
        let id2 = SlackChannel::message_id("C12345", "1234567890.123457");
        assert_ne!(id1, id2);
    }

    #[test]
    fn slack_message_id_different_channel_different_id() {
        let id1 = SlackChannel::message_id("C12345", "1234567890.123456");
        let id2 = SlackChannel::message_id("C67890", "1234567890.123456");
        assert_ne!(id1, id2);
    }

    #[test]
    fn slack_message_id_no_uuid_randomness() {
        let id = SlackChannel::message_id("C12345", "1234567890.123456");
        assert!(!id.contains('-'));
        assert!(id.starts_with("slack_"));
    }

    #[test]
    fn inbound_thread_ts_prefers_explicit_thread_ts() {
        let msg = serde_json::json!({
            "ts": "123.002",
            "thread_ts": "123.001"
        });

        let thread_ts = SlackChannel::inbound_thread_ts(&msg, "123.002");
        assert_eq!(thread_ts.as_deref(), Some("123.001"));
    }

    #[test]
    fn inbound_thread_ts_falls_back_to_ts() {
        let msg = serde_json::json!({
            "ts": "123.001"
        });

        let thread_ts = SlackChannel::inbound_thread_ts(&msg, "123.001");
        assert_eq!(thread_ts.as_deref(), Some("123.001"));
    }

    #[test]
    fn inbound_thread_ts_none_when_ts_missing() {
        let msg = serde_json::json!({});

        let thread_ts = SlackChannel::inbound_thread_ts(&msg, "");
        assert_eq!(thread_ts, None);
    }

    // ── Socket Mode envelope parsing ──────────────────────────────

    #[test]
    fn parse_envelope_events_api() {
        let json = r#"{
            "envelope_id": "abc-123",
            "type": "events_api",
            "payload": {
                "event": {
                    "type": "message",
                    "user": "U123",
                    "text": "hello",
                    "channel": "C456",
                    "ts": "1234567890.000001"
                }
            }
        }"#;
        let env = SlackChannel::parse_envelope(json).unwrap();
        assert_eq!(env.envelope_id, "abc-123");
        assert_eq!(env.envelope_type, "events_api");
        assert!(env.payload.get("event").is_some());
    }

    #[test]
    fn parse_envelope_interactive() {
        let json = r#"{
            "envelope_id": "def-456",
            "type": "interactive",
            "payload": {
                "type": "block_actions",
                "actions": [{"action_id": "btn_1"}]
            }
        }"#;
        let env = SlackChannel::parse_envelope(json).unwrap();
        assert_eq!(env.envelope_id, "def-456");
        assert_eq!(env.envelope_type, "interactive");
    }

    #[test]
    fn parse_envelope_missing_fields() {
        let json = r#"{"not_an_envelope": true}"#;
        assert!(SlackChannel::parse_envelope(json).is_err());
    }

    #[test]
    fn parse_envelope_hello_not_an_envelope() {
        // hello messages lack envelope_id/payload — should fail parse_envelope
        let json = r#"{"type": "hello", "connection_info": {"app_id": "A123"}}"#;
        assert!(SlackChannel::parse_envelope(json).is_err());
    }

    #[test]
    fn parse_envelope_disconnect_not_an_envelope() {
        // disconnect messages lack envelope_id/payload — should fail parse_envelope
        let json = r#"{"type": "disconnect", "reason": "link_disabled"}"#;
        assert!(SlackChannel::parse_envelope(json).is_err());
    }

    // ── WSS URL validation ────────────────────────────────────────

    #[test]
    fn validate_wss_url_accepts_slack_domain() {
        assert!(SlackChannel::validate_wss_url("wss://wss-primary.slack.com/link").is_ok());
        assert!(SlackChannel::validate_wss_url("wss://cerberus-xxl.lb.slack.com/foo").is_ok());
    }

    #[test]
    fn validate_wss_url_rejects_non_wss_scheme() {
        assert!(SlackChannel::validate_wss_url("ws://wss-primary.slack.com/link").is_err());
        assert!(SlackChannel::validate_wss_url("http://wss-primary.slack.com/link").is_err());
    }

    #[test]
    fn validate_wss_url_rejects_non_slack_host() {
        assert!(SlackChannel::validate_wss_url("wss://evil.com/link").is_err());
        assert!(SlackChannel::validate_wss_url("wss://notslack.com/link").is_err());
        // Domain boundary check: evil-slack.com must not pass
        assert!(SlackChannel::validate_wss_url("wss://evil-slack.com/link").is_err());
    }

    // ── Event message extraction ──────────────────────────────────

    #[test]
    fn extract_event_message_normal() {
        let payload = serde_json::json!({
            "event": {
                "type": "message",
                "user": "U123",
                "text": "hello world",
                "channel": "C456",
                "ts": "1234567890.000001"
            }
        });
        let result = SlackChannel::extract_event_message(&payload, "BXXX");
        assert!(result.is_some());
        let (user, text, channel, ts, thread_ts) = result.unwrap();
        assert_eq!(user, "U123");
        assert_eq!(text, "hello world");
        assert_eq!(channel, "C456");
        assert_eq!(ts, "1234567890.000001");
        assert_eq!(thread_ts.as_deref(), Some("1234567890.000001"));
    }

    #[test]
    fn extract_event_message_threaded_reply() {
        let payload = serde_json::json!({
            "event": {
                "type": "message",
                "user": "U123",
                "text": "reply in thread",
                "channel": "C456",
                "ts": "1234567890.000010",
                "thread_ts": "1234567890.000001"
            }
        });
        let result = SlackChannel::extract_event_message(&payload, "BXXX");
        assert!(result.is_some());
        let (_user, _text, _channel, ts, thread_ts) = result.unwrap();
        assert_eq!(ts, "1234567890.000010");
        // thread_ts should be the parent thread, not the message ts
        assert_eq!(thread_ts.as_deref(), Some("1234567890.000001"));
    }

    #[test]
    fn extract_event_message_app_mention() {
        let payload = serde_json::json!({
            "event": {
                "type": "app_mention",
                "user": "U789",
                "text": "<@BXXX> do something",
                "channel": "C456",
                "ts": "1234567890.000002"
            }
        });
        let result = SlackChannel::extract_event_message(&payload, "BXXX");
        assert!(result.is_some());
        let (user, text, _channel, _ts, _thread_ts) = result.unwrap();
        assert_eq!(user, "U789");
        assert_eq!(text, "<@BXXX> do something");
    }

    #[test]
    fn extract_event_message_skip_bot() {
        let payload = serde_json::json!({
            "event": {
                "type": "message",
                "user": "U123",
                "text": "from bot",
                "channel": "C456",
                "ts": "1234567890.000003",
                "bot_id": "B999"
            }
        });
        assert!(SlackChannel::extract_event_message(&payload, "BXXX").is_none());
    }

    #[test]
    fn extract_event_message_skip_own_bot() {
        let payload = serde_json::json!({
            "event": {
                "type": "message",
                "user": "BXXX",
                "text": "my own message",
                "channel": "C456",
                "ts": "1234567890.000004"
            }
        });
        assert!(SlackChannel::extract_event_message(&payload, "BXXX").is_none());
    }

    #[test]
    fn extract_event_message_skip_subtype() {
        let payload = serde_json::json!({
            "event": {
                "type": "message",
                "subtype": "message_changed",
                "user": "U123",
                "text": "edited",
                "channel": "C456",
                "ts": "1234567890.000005"
            }
        });
        assert!(SlackChannel::extract_event_message(&payload, "BXXX").is_none());

        let payload2 = serde_json::json!({
            "event": {
                "type": "message",
                "subtype": "message_deleted",
                "user": "U123",
                "text": "deleted",
                "channel": "C456",
                "ts": "1234567890.000006"
            }
        });
        assert!(SlackChannel::extract_event_message(&payload2, "BXXX").is_none());
    }

    // ── Send body building (tests actual build_send_body) ─────────

    #[test]
    fn build_send_body_includes_identity_fields() {
        let msg = SendMessage::new("hello", "C123")
            .with_identity(Some("PM Bot".into()), Some(":robot_face:".into()));
        let body = SlackChannel::build_send_body(&msg);
        assert_eq!(body["username"], "PM Bot");
        assert_eq!(body["icon_emoji"], ":robot_face:");
        assert_eq!(body["channel"], "C123");
        assert_eq!(body["text"], "hello");
    }

    #[test]
    fn build_send_body_includes_blocks() {
        let blocks =
            serde_json::json!([{"type": "section", "text": {"type": "mrkdwn", "text": "hi"}}]);
        let msg = SendMessage::new("fallback", "C123").with_blocks(blocks.clone());
        let body = SlackChannel::build_send_body(&msg);
        assert_eq!(body["blocks"], blocks);
        assert_eq!(body["text"], "fallback");
    }

    #[test]
    fn build_send_body_includes_reply_broadcast() {
        let msg = SendMessage::new("hello", "C123")
            .in_thread(Some("ts123".into()))
            .with_reply_broadcast(true);
        let body = SlackChannel::build_send_body(&msg);
        assert_eq!(body["reply_broadcast"], true);
        assert_eq!(body["thread_ts"], "ts123");
    }

    #[test]
    fn build_send_body_omits_none_fields() {
        let msg = SendMessage::new("hello", "C123");
        let body = SlackChannel::build_send_body(&msg);
        assert!(body.get("username").is_none());
        assert!(body.get("icon_emoji").is_none());
        assert!(body.get("blocks").is_none());
        assert!(body.get("reply_broadcast").is_none());
        assert!(body.get("thread_ts").is_none());
    }

    // ── Interactive flow — Block Kit template builders ─────────────

    #[test]
    fn build_issue_draft_blocks_has_three_action_buttons() {
        let blocks = SlackChannel::build_issue_draft_blocks("Fix auth bug", "Auth fails on mobile");
        let elements = blocks[1]["elements"].as_array().unwrap();
        assert_eq!(elements.len(), 3);
    }

    #[test]
    fn build_issue_draft_blocks_action_ids_are_correct() {
        let blocks = SlackChannel::build_issue_draft_blocks("Fix auth bug", "desc");
        let elements = blocks[1]["elements"].as_array().unwrap();
        assert_eq!(elements[0]["action_id"], BK_ACTION_CONFIRM);
        assert_eq!(elements[1]["action_id"], BK_ACTION_EDIT);
        assert_eq!(elements[2]["action_id"], BK_ACTION_CANCEL);
    }

    #[test]
    fn build_issue_draft_blocks_confirm_is_primary_cancel_is_danger() {
        let blocks = SlackChannel::build_issue_draft_blocks("T", "D");
        let elements = blocks[1]["elements"].as_array().unwrap();
        assert_eq!(elements[0]["style"], "primary");
        assert_eq!(elements[2]["style"], "danger");
        // Edit button has no style override
        assert!(elements[1].get("style").map_or(true, |v| v.is_null()));
    }

    #[test]
    fn build_issue_draft_blocks_title_in_section_text() {
        let blocks = SlackChannel::build_issue_draft_blocks("Auth Bug", "Fix mobile auth");
        let section_text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(section_text.contains("Auth Bug"));
        assert!(section_text.contains("Fix mobile auth"));
    }

    #[test]
    fn build_issue_draft_blocks_title_as_button_value() {
        let blocks = SlackChannel::build_issue_draft_blocks("Auth Bug", "desc");
        let elements = blocks[1]["elements"].as_array().unwrap();
        // confirm and edit carry the title as value for handler context recovery
        assert_eq!(elements[0]["value"], "Auth Bug");
        assert_eq!(elements[1]["value"], "Auth Bug");
    }

    #[test]
    fn build_issue_confirmation_blocks_contains_link() {
        let blocks = SlackChannel::build_issue_confirmation_blocks(
            "Auth Bug",
            "https://linear.app/team/issue/TEAM-1",
        );
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("https://linear.app/team/issue/TEAM-1"));
        assert!(text.contains("Auth Bug"));
    }

    #[test]
    fn build_issue_confirmation_blocks_is_mrkdwn_section() {
        let blocks = SlackChannel::build_issue_confirmation_blocks("T", "https://example.com");
        assert_eq!(blocks[0]["type"], "section");
        assert_eq!(blocks[0]["text"]["type"], "mrkdwn");
    }

    #[test]
    fn build_issue_modal_has_required_structure() {
        let modal = SlackChannel::build_issue_modal("Fix auth", "C123:1234567890.000001");
        assert_eq!(modal["type"], "modal");
        assert_eq!(modal["callback_id"], BK_CALLBACK_EDIT_MODAL);
        assert_eq!(modal["private_metadata"], "C123:1234567890.000001");
        let blocks = modal["blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["block_id"], BK_BLOCK_TITLE);
        assert_eq!(blocks[1]["block_id"], BK_BLOCK_DESCRIPTION);
    }

    #[test]
    fn build_issue_modal_prefills_title() {
        let modal = SlackChannel::build_issue_modal("Prefill Title", "C123:ts");
        let title_input = &modal["blocks"][0]["element"];
        assert_eq!(title_input["initial_value"], "Prefill Title");
    }

    // ── Interactive flow — block_action payload parsing ───────────

    #[test]
    fn parse_block_action_context_extracts_all_fields() {
        let payload = serde_json::json!({
            "user": {"id": "U123"},
            "channel": {"id": "C456"},
            "message": {"ts": "1234567890.000001", "thread_ts": "1234567890.000000"},
            "actions": [{"action_id": "confirm_issue", "value": "Fix auth bug"}],
            "trigger_id": "TRG1"
        });
        let result = SlackChannel::parse_block_action_context(&payload);
        assert!(result.is_some());
        let (user, channel, thread_ts, action_id, value, trigger_id) = result.unwrap();
        assert_eq!(user, "U123");
        assert_eq!(channel, "C456");
        assert_eq!(thread_ts.as_deref(), Some("1234567890.000000"));
        assert_eq!(action_id, "confirm_issue");
        assert_eq!(value, "Fix auth bug");
        assert_eq!(trigger_id, "TRG1");
    }

    #[test]
    fn parse_block_action_context_missing_user_returns_none() {
        let payload = serde_json::json!({
            "channel": {"id": "C456"},
            "actions": [{"action_id": "confirm_issue", "value": "title"}],
            "trigger_id": "TRG1"
        });
        assert!(SlackChannel::parse_block_action_context(&payload).is_none());
    }

    #[test]
    fn parse_block_action_context_missing_channel_returns_none() {
        let payload = serde_json::json!({
            "user": {"id": "U123"},
            "actions": [{"action_id": "confirm_issue", "value": "title"}],
            "trigger_id": "TRG1"
        });
        assert!(SlackChannel::parse_block_action_context(&payload).is_none());
    }

    #[test]
    fn parse_block_action_context_empty_actions_returns_none() {
        let payload = serde_json::json!({
            "user": {"id": "U123"},
            "channel": {"id": "C456"},
            "actions": [],
            "trigger_id": "TRG1"
        });
        assert!(SlackChannel::parse_block_action_context(&payload).is_none());
    }

    #[test]
    fn parse_block_action_context_thread_ts_falls_back_to_message_ts() {
        let payload = serde_json::json!({
            "user": {"id": "U123"},
            "channel": {"id": "C456"},
            "message": {"ts": "1234567890.000001"},
            "actions": [{"action_id": "confirm_issue", "value": "title"}],
            "trigger_id": "TRG1"
        });
        let (_, _, thread_ts, _, _, _) =
            SlackChannel::parse_block_action_context(&payload).unwrap();
        assert_eq!(thread_ts.as_deref(), Some("1234567890.000001"));
    }

    #[test]
    fn parse_block_action_context_no_message_gives_none_thread_ts() {
        let payload = serde_json::json!({
            "user": {"id": "U123"},
            "channel": {"id": "C456"},
            "actions": [{"action_id": "confirm_issue", "value": "title"}],
            "trigger_id": "TRG1"
        });
        let (_, _, thread_ts, _, _, _) =
            SlackChannel::parse_block_action_context(&payload).unwrap();
        assert!(thread_ts.is_none());
    }

    #[test]
    fn parse_block_action_context_missing_action_id_returns_none() {
        let payload = serde_json::json!({
            "user": {"id": "U123"},
            "channel": {"id": "C456"},
            "actions": [{"value": "title"}],
            "trigger_id": "TRG1"
        });
        assert!(SlackChannel::parse_block_action_context(&payload).is_none());
    }

    // ── Async handler tests ───────────────────────────────────────

    fn wildcard_channel() -> SlackChannel {
        SlackChannel::new("xoxb-fake".into(), None, None, vec!["*".into()])
    }

    fn block_action_payload(action_id: &str, value: &str) -> serde_json::Value {
        serde_json::json!({
            "user": {"id": "U123"},
            "channel": {"id": "C456"},
            "actions": [{"action_id": action_id, "value": value}],
            "trigger_id": "TRG1"
        })
    }

    #[tokio::test]
    async fn block_action_confirm_forwards_message() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = block_action_payload(BK_ACTION_CONFIRM, "Fix auth bug");
        ch.handle_block_action(&payload, None, &tx).await.unwrap();
        let msg = rx.try_recv().expect("confirm should forward message");
        assert!(msg.content.contains(BK_ACTION_CONFIRM));
        assert!(msg.content.contains("Fix auth bug"));
        assert_eq!(msg.channel, "slack");
        assert_eq!(msg.reply_target, "C456");
    }

    #[tokio::test]
    async fn block_action_value_bracket_characters_stripped() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = block_action_payload(BK_ACTION_CONFIRM, "[injected] value [end]");
        ch.handle_block_action(&payload, None, &tx).await.unwrap();
        let msg = rx.try_recv().expect("confirm should forward message");
        // The format is "[block_action:{id}] {safe_value}".
        // Split on the closing "] " to isolate the value portion.
        let value_part = msg.content.splitn(2, "] ").nth(1).unwrap_or("");
        assert!(
            !value_part.contains('['),
            "[ must be stripped from button value"
        );
        assert!(
            !value_part.contains(']'),
            "] must be stripped from button value"
        );
        assert!(
            value_part.contains("injected"),
            "text content must survive stripping"
        );
        assert!(
            value_part.contains("value"),
            "text content must survive stripping"
        );
    }

    #[tokio::test]
    async fn block_action_cancel_forwards_message() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = block_action_payload(BK_ACTION_CANCEL, "");
        ch.handle_block_action(&payload, None, &tx).await.unwrap();
        let msg = rx.try_recv().expect("cancel should forward message");
        assert!(msg.content.contains(BK_ACTION_CANCEL));
    }

    #[tokio::test]
    async fn block_action_unknown_action_id_dropped() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = block_action_payload("unknown_action", "val");
        ch.handle_block_action(&payload, None, &tx).await.unwrap();
        assert!(
            rx.try_recv().is_err(),
            "unknown action_id should be dropped, not forwarded"
        );
    }

    #[test]
    fn escape_mrkdwn_replaces_special_chars() {
        assert_eq!(SlackChannel::escape_mrkdwn("a & b"), "a &amp; b");
        assert_eq!(SlackChannel::escape_mrkdwn("<@U123>"), "&lt;@U123&gt;");
        assert_eq!(
            SlackChannel::escape_mrkdwn("<https://evil.com|click>"),
            "&lt;https://evil.com|click&gt;"
        );
        assert_eq!(SlackChannel::escape_mrkdwn("no specials"), "no specials");
    }

    #[test]
    fn build_issue_draft_blocks_escapes_mrkdwn_in_title_and_description() {
        let blocks = SlackChannel::build_issue_draft_blocks("<@U999> attack", "& <script>");
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("&lt;@U999&gt;"), "< and > should be escaped");
        assert!(text.contains("&amp;"), "& should be escaped");
        assert!(!text.contains("<@U999>"), "raw mention must not appear");
    }

    #[test]
    fn build_issue_confirmation_blocks_escapes_title() {
        let blocks = SlackChannel::build_issue_confirmation_blocks(
            "<Attack>",
            "https://linear.app/t/TEAM-1",
        );
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("&lt;Attack&gt;"));
        assert!(!text.contains("<Attack>"));
    }

    #[test]
    fn build_issue_confirmation_blocks_url_pipe_injection_prevented() {
        // A URL containing `|` would let an attacker inject display text in
        // Slack's mrkdwn link format `<url|text>`. The `|` must be removed.
        let blocks = SlackChannel::build_issue_confirmation_blocks(
            "Real Title",
            "https://linear.app/t/TEAM-1|hacked display text",
        );
        let text = blocks[0]["text"]["text"].as_str().unwrap();
        assert!(
            !text.contains("|hacked display text"),
            "pipe injection must be neutralised"
        );
        assert!(text.contains("Real Title"), "real title must still appear");
    }

    #[tokio::test]
    async fn block_action_unauthorized_user_skipped() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec!["U999".into()]);
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = block_action_payload(BK_ACTION_CONFIRM, "title");
        ch.handle_block_action(&payload, None, &tx).await.unwrap();
        assert!(
            rx.try_recv().is_err(),
            "unauthorized user should not forward"
        );
    }

    #[tokio::test]
    async fn block_action_scoped_channel_filters_wrong_channel() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = block_action_payload(BK_ACTION_CONFIRM, "title");
        ch.handle_block_action(&payload, Some("C999"), &tx)
            .await
            .unwrap();
        assert!(rx.try_recv().is_err(), "wrong channel should be filtered");
    }

    #[tokio::test]
    async fn block_action_missing_fields_skipped() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = serde_json::json!({"type": "block_actions"});
        ch.handle_block_action(&payload, None, &tx).await.unwrap();
        assert!(rx.try_recv().is_err(), "missing fields should be skipped");
    }

    fn view_submission_payload(
        private_metadata: &str,
        title: &str,
        description: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "user": {"id": "U123"},
            "view": {
                "callback_id": BK_CALLBACK_EDIT_MODAL,
                "private_metadata": private_metadata,
                "state": {
                    "values": {
                        BK_BLOCK_TITLE: {
                            BK_INPUT_TITLE: {"value": title}
                        },
                        BK_BLOCK_DESCRIPTION: {
                            BK_INPUT_DESCRIPTION: {"value": description}
                        }
                    }
                }
            }
        })
    }

    #[tokio::test]
    async fn view_submission_forwards_form_values() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload =
            view_submission_payload("C456:1234567890.000001", "My issue title", "My description");
        ch.handle_view_submission(&payload, None, &tx)
            .await
            .unwrap();
        let msg = rx
            .try_recv()
            .expect("view submission should forward message");
        assert!(msg.content.contains("My issue title"));
        assert!(msg.content.contains("My description"));
        assert_eq!(msg.reply_target, "C456");
        assert_eq!(msg.thread_ts.as_deref(), Some("1234567890.000001"));
        assert_eq!(msg.channel, "slack");
    }

    #[tokio::test]
    async fn view_submission_without_thread_ts_uses_channel_only() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        // No ":" in private_metadata → channel only, thread_ts = None
        let payload = view_submission_payload("C456", "Title", "Desc");
        ch.handle_view_submission(&payload, None, &tx)
            .await
            .unwrap();
        let msg = rx
            .try_recv()
            .expect("view submission should forward message");
        assert_eq!(msg.reply_target, "C456");
        assert!(msg.thread_ts.is_none());
    }

    #[tokio::test]
    async fn view_submission_missing_view_skipped() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = serde_json::json!({"user": {"id": "U123"}});
        ch.handle_view_submission(&payload, None, &tx)
            .await
            .unwrap();
        assert!(rx.try_recv().is_err(), "missing view should be skipped");
    }

    #[tokio::test]
    async fn view_submission_empty_private_metadata_skipped() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = view_submission_payload("", "Title", "Desc");
        ch.handle_view_submission(&payload, None, &tx)
            .await
            .unwrap();
        assert!(rx.try_recv().is_err(), "empty metadata should be skipped");
    }

    #[tokio::test]
    async fn view_submission_scoped_channel_filters_wrong_channel() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = view_submission_payload("C456:ts", "Title", "Desc");
        ch.handle_view_submission(&payload, Some("C999"), &tx)
            .await
            .unwrap();
        assert!(
            rx.try_recv().is_err(),
            "wrong scoped channel should be filtered"
        );
    }

    #[tokio::test]
    async fn view_submission_scoped_channel_passes_correct_channel() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = view_submission_payload("C456:ts", "Title", "Desc");
        ch.handle_view_submission(&payload, Some("C456"), &tx)
            .await
            .unwrap();
        assert!(rx.try_recv().is_ok(), "correct scoped channel should pass");
    }

    #[tokio::test]
    async fn view_submission_unauthorized_user_skipped() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec!["U999".into()]);
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let payload = view_submission_payload("C456:ts", "Title", "Desc");
        ch.handle_view_submission(&payload, None, &tx)
            .await
            .unwrap();
        assert!(
            rx.try_recv().is_err(),
            "unauthorized user should be skipped"
        );
    }

    // ── M-7: dispatch_envelope ────────────────────────────────────

    fn events_api_envelope(user: &str, channel: &str, ts: &str, text: &str) -> SocketModeEnvelope {
        SocketModeEnvelope {
            envelope_id: "env-1".to_string(),
            envelope_type: "events_api".to_string(),
            payload: serde_json::json!({
                "event": {
                    "type": "message",
                    "user": user,
                    "text": text,
                    "channel": channel,
                    "ts": ts
                }
            }),
        }
    }

    #[tokio::test]
    async fn dispatch_envelope_events_api_forwards_message() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let env = events_api_envelope("U123", "C456", "1234567890.000001", "hello");
        ch.dispatch_envelope(env, "BBOT", None, &tx).await.unwrap();
        let msg = rx
            .try_recv()
            .expect("events_api message should be forwarded");
        assert_eq!(msg.sender, "U123");
        assert_eq!(msg.content, "hello");
        assert_eq!(msg.reply_target, "C456");
        assert_eq!(msg.channel, "slack");
        // thread_ts missing from event — falls back to ts
        assert_eq!(msg.thread_ts, Some("1234567890.000001".to_string()));
    }

    #[tokio::test]
    async fn dispatch_envelope_events_api_scoped_channel_mismatch_discarded() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let env = events_api_envelope("U123", "C456", "1234567890.000001", "hello");
        ch.dispatch_envelope(env, "BBOT", Some("C789"), &tx)
            .await
            .unwrap();
        assert!(
            rx.try_recv().is_err(),
            "message to wrong scoped channel should be discarded"
        );
    }

    #[tokio::test]
    async fn dispatch_envelope_events_api_unauthorized_user_discarded() {
        let ch = SlackChannel::new("xoxb-fake".into(), None, None, vec!["U999".into()]);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let env = events_api_envelope("U123", "C456", "1234567890.000001", "hello");
        ch.dispatch_envelope(env, "BBOT", None, &tx).await.unwrap();
        assert!(
            rx.try_recv().is_err(),
            "message from unauthorized user should be discarded"
        );
    }

    #[tokio::test]
    async fn dispatch_envelope_interactive_block_action_forwarded() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let env = SocketModeEnvelope {
            envelope_id: "env-2".to_string(),
            envelope_type: "interactive".to_string(),
            payload: serde_json::json!({
                "type": "block_actions",
                "user": {"id": "U123"},
                "channel": {"id": "C456"},
                "actions": [{"action_id": "confirm_issue", "value": "Auth Bug"}],
                "trigger_id": "TRG1"
            }),
        };
        ch.dispatch_envelope(env, "BBOT", None, &tx).await.unwrap();
        let msg = rx
            .try_recv()
            .expect("block_action confirm should be forwarded");
        assert!(msg.content.contains("confirm_issue"));
    }

    #[tokio::test]
    async fn dispatch_envelope_slash_commands_discarded() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let env = SocketModeEnvelope {
            envelope_id: "env-3".to_string(),
            envelope_type: "slash_commands".to_string(),
            payload: serde_json::json!({"command": "/pm", "text": "help"}),
        };
        ch.dispatch_envelope(env, "BBOT", None, &tx).await.unwrap();
        assert!(
            rx.try_recv().is_err(),
            "slash commands should be discarded without forwarding"
        );
    }

    #[tokio::test]
    async fn dispatch_envelope_unknown_type_discarded() {
        let ch = wildcard_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let env = SocketModeEnvelope {
            envelope_id: "env-4".to_string(),
            envelope_type: "mystery_envelope".to_string(),
            payload: serde_json::json!({}),
        };
        ch.dispatch_envelope(env, "BBOT", None, &tx).await.unwrap();
        assert!(
            rx.try_recv().is_err(),
            "unknown envelope type should be silently discarded"
        );
    }
}
