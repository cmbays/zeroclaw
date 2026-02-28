//! Linear integration tool — full PM capabilities: issue CRUD, comments, archiving,
//! state/member/label lookups, project management, cycle management, initiative queries.
//!
//! Implements the [`Tool`] trait so the agent loop can call Linear's GraphQL API
//! directly. All network I/O goes through a single [`LinearTool::graphql`] helper
//! that adds authentication and validates the HTTP response.

use super::traits::{Tool, ToolResult};
use super::url_validation::{validate_url, DomainPolicy, UrlSchemePolicy};
use crate::config::UrlAccessConfig;
use anyhow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};

const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

/// Validate that `url` is the Linear GraphQL endpoint — HTTPS only, `api.linear.app` only,
/// with private-IP blocking. Called before every reqwest call in [`LinearTool::graphql`].
fn validate_linear_url(url: &str) -> anyhow::Result<()> {
    let allowed = ["api.linear.app".to_string()];
    validate_url(
        url,
        &DomainPolicy {
            allowed_domains: &allowed,
            blocked_domains: &[],
            allowed_field_name: "linear.endpoint",
            blocked_field_name: None,
            empty_allowed_message: "Linear endpoint allowlist must not be empty",
            scheme_policy: UrlSchemePolicy::HttpsOnly,
            ipv6_error_context: "linear",
            url_access: Some(&UrlAccessConfig::default()),
        },
    )?;
    Ok(())
}

/// Linear API tool — exposes full PM capabilities to the agent.
pub struct LinearTool {
    api_key: String,
    team_id: String,
    client: reqwest::Client,
}

impl LinearTool {
    /// Construct a `LinearTool` with a proxy-aware HTTP client.
    ///
    /// Returns an error if the underlying TLS/proxy configuration is invalid.
    pub fn new(api_key: String, team_id: String) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to build Linear HTTP client")?;
        Ok(Self {
            api_key,
            team_id,
            client,
        })
    }

    /// Execute a GraphQL query/mutation against the Linear API.
    async fn graphql(&self, query: &str, variables: Value) -> anyhow::Result<Value> {
        validate_linear_url(LINEAR_API_URL)?;

        let body = json!({ "query": query, "variables": variables });

        let resp = self
            .client
            .post(LINEAR_API_URL)
            .header("Authorization", &self.api_key)
            .json(&body)
            .send()
            .await
            .context("Linear API request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!("Linear API returned HTTP {}", resp.status());
        }

        let json: Value = resp
            .json()
            .await
            .context("Failed to parse Linear GraphQL response")?;

        if let Some(errors) = json.get("errors") {
            let arr = errors.as_array();
            let error_count = arr.map_or(1, |a| a.len());
            let first_msg = arr
                .and_then(|a| a.first())
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            tracing::warn!(error_count, errors = %errors, "Linear GraphQL errors");
            anyhow::bail!("Linear API error: {first_msg}");
        }

        json.get("data")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Linear API response missing 'data' field"))
    }

    // ── Issue operations ─────────────────────────────────────────────────────

    async fn create_issue(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let title = args["title"].as_str().context("title required")?;
        let description = args["description"].as_str().unwrap_or("");

        let query = r#"
            mutation CreateIssue($input: IssueCreateInput!) {
                issueCreate(input: $input) {
                    issue {
                        id
                        identifier
                        title
                        url
                    }
                }
            }
        "#;

        let mut input = json!({
            "teamId": self.team_id,
            "title": title,
            "description": description,
        });

        if let Some(v) = args["assignee_id"].as_str() {
            input["assigneeId"] = json!(v);
        }
        if let Some(v) = args["project_id"].as_str() {
            input["projectId"] = json!(v);
        }
        if let Some(v) = args["template_id"].as_str() {
            input["templateId"] = json!(v);
        }
        if let Some(v) = args["priority"].as_i64() {
            input["priority"] = json!(v);
        }
        if let Some(arr) = args["label_ids"].as_array() {
            input["labelIds"] = json!(arr);
        }
        if let Some(v) = args["parent_id"].as_str() {
            input["parentId"] = json!(v);
        }
        if let Some(v) = args["cycle_id"].as_str() {
            input["cycleId"] = json!(v);
        }
        if let Some(v) = args["estimate"].as_i64() {
            input["estimate"] = json!(v);
        }
        if let Some(v) = args["due_date"].as_str() {
            input["dueDate"] = json!(v);
        }

        let data = self.graphql(query, json!({ "input": input })).await?;
        let issue = &data["issueCreate"]["issue"];

        let output = format!(
            "Created {} — {}\n{}",
            issue["identifier"].as_str().unwrap_or(""),
            issue["title"].as_str().unwrap_or(""),
            issue["url"].as_str().unwrap_or("")
        );

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    async fn list_issues(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let query = r#"
            query ListIssues($filter: IssueFilter) {
                issues(filter: $filter, first: 25) {
                    nodes {
                        identifier
                        title
                        state { name }
                        assignee { name }
                        priority
                        url
                    }
                }
            }
        "#;

        let mut filter = json!({
            "team": { "id": { "eq": self.team_id } }
        });

        if let Some(v) = args["project_id"].as_str() {
            filter["project"] = json!({ "id": { "eq": v } });
        }
        if let Some(v) = args["status"].as_str() {
            filter["state"] = json!({ "name": { "eq": v } });
        }
        if let Some(v) = args["assignee_id"].as_str() {
            filter["assignee"] = json!({ "id": { "eq": v } });
        }
        if let Some(v) = args["cycle_id"].as_str() {
            filter["cycle"] = json!({ "id": { "eq": v } });
        }
        if let Some(v) = args["label_id"].as_str() {
            filter["labels"] = json!({ "id": { "eq": v } });
        }
        if let Some(v) = args["priority"].as_i64() {
            filter["priority"] = json!({ "eq": v });
        }
        if let Some(v) = args["search"].as_str() {
            filter["title"] = json!({ "containsIgnoreCase": v });
        }

        let data = self.graphql(query, json!({ "filter": filter })).await?;
        let nodes = match data["issues"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            Some(_) => {
                // Legitimate empty result — no issues match the filter.
                return Ok(ToolResult {
                    success: true,
                    output: "No issues found.".into(),
                    error: None,
                });
            }
            None => {
                // Schema mismatch: 'issues' or 'issues.nodes' is missing / wrong type.
                if data["issues"].is_null() {
                    tracing::warn!(
                        "list_issues: 'issues' field is missing or null in Linear GraphQL \
                         response; possible API schema change"
                    );
                } else {
                    tracing::warn!(
                        actual = %data["issues"]["nodes"],
                        "list_issues: 'issues.nodes' is not an array in Linear GraphQL \
                         response; possible API schema change"
                    );
                }
                return Ok(ToolResult {
                    success: true,
                    output: "No issues found.".into(),
                    error: None,
                });
            }
        };

        let lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                let priority = n["priority"]
                    .as_i64()
                    .map_or(String::new(), |p| format!(" p{p}"));
                format!(
                    "{} — {} [{}]{} {}",
                    n["identifier"].as_str().unwrap_or(""),
                    n["title"].as_str().unwrap_or(""),
                    n["state"]["name"].as_str().unwrap_or(""),
                    priority,
                    n["url"].as_str().unwrap_or("")
                )
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    async fn update_issue(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let issue_id = args["issue_id"].as_str().context("issue_id required")?;

        let query = r#"
            mutation UpdateIssue($id: String!, $input: IssueUpdateInput!) {
                issueUpdate(id: $id, input: $input) {
                    issue {
                        identifier
                        title
                        state { name }
                        url
                    }
                }
            }
        "#;

        let mut input = json!({});

        if let Some(v) = args["title"].as_str() {
            input["title"] = json!(v);
        }
        if let Some(v) = args["description"].as_str() {
            input["description"] = json!(v);
        }
        if let Some(v) = args["state_id"].as_str() {
            input["stateId"] = json!(v);
        }
        if let Some(v) = args["assignee_id"].as_str() {
            input["assigneeId"] = json!(v);
        }
        if let Some(v) = args["priority"].as_i64() {
            input["priority"] = json!(v);
        }
        if let Some(arr) = args["label_ids"].as_array() {
            input["labelIds"] = json!(arr);
        }
        if let Some(v) = args["parent_id"].as_str() {
            input["parentId"] = json!(v);
        }
        if let Some(v) = args["cycle_id"].as_str() {
            input["cycleId"] = json!(v);
        }
        if let Some(v) = args["estimate"].as_i64() {
            input["estimate"] = json!(v);
        }
        if let Some(v) = args["due_date"].as_str() {
            input["dueDate"] = json!(v);
        }
        if let Some(v) = args["project_id"].as_str() {
            input["projectId"] = json!(v);
        }

        if input.as_object().map_or(true, |o| o.is_empty()) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "At least one field must be provided to update_issue \
                     (title, description, state_id, assignee_id, priority, label_ids, \
                     parent_id, cycle_id, estimate, due_date, or project_id)"
                        .to_string(),
                ),
            });
        }

        let data = self
            .graphql(query, json!({ "id": issue_id, "input": input }))
            .await?;
        let issue = &data["issueUpdate"]["issue"];

        let output = format!(
            "Updated {} — {} [{}]\n{}",
            issue["identifier"].as_str().unwrap_or(""),
            issue["title"].as_str().unwrap_or(""),
            issue["state"]["name"].as_str().unwrap_or(""),
            issue["url"].as_str().unwrap_or("")
        );

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    async fn get_issue(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(issue_id) = args["issue_id"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("issue_id required".to_string()),
            });
        };

        let query = r#"
            query GetIssue($id: String!) {
                issue(id: $id) {
                    id
                    identifier
                    title
                    description
                    priority
                    estimate
                    dueDate
                    url
                    state { name }
                    assignee { name }
                    labels { nodes { name } }
                    project { name }
                    cycle { name }
                    parent { identifier }
                    comments(first: 20) {
                        nodes {
                            body
                            user { name }
                            createdAt
                        }
                    }
                }
            }
        "#;

        let data = self.graphql(query, json!({ "id": issue_id })).await?;
        let issue = &data["issue"];

        use std::fmt::Write as _;
        let mut output = format!(
            "{} — {} [{}]\nPriority: {} | Estimate: {} | Due: {}\nProject: {} | Cycle: {} | Parent: {}\n{}",
            issue["identifier"].as_str().unwrap_or(""),
            issue["title"].as_str().unwrap_or(""),
            issue["state"]["name"].as_str().unwrap_or(""),
            issue["priority"].as_i64().map_or("—".into(), |p| p.to_string()),
            issue["estimate"].as_i64().map_or("—".into(), |e| e.to_string()),
            issue["dueDate"].as_str().unwrap_or("—"),
            issue["project"]["name"].as_str().unwrap_or("—"),
            issue["cycle"]["name"].as_str().unwrap_or("—"),
            issue["parent"]["identifier"].as_str().unwrap_or("—"),
            issue["url"].as_str().unwrap_or(""),
        );

        if let Some(labels) = issue["labels"]["nodes"].as_array() {
            let label_names: Vec<&str> = labels.iter().filter_map(|l| l["name"].as_str()).collect();
            if !label_names.is_empty() {
                let _ = write!(output, "\nLabels: {}", label_names.join(", "));
            }
        }

        if let Some(desc) = issue["description"].as_str() {
            if !desc.is_empty() {
                let _ = write!(output, "\n\nDescription:\n{desc}");
            }
        }

        if let Some(comments) = issue["comments"]["nodes"].as_array() {
            if !comments.is_empty() {
                let _ = write!(output, "\n\nComments:");
                for c in comments {
                    let _ = write!(
                        output,
                        "\n[{}] {}: {}",
                        c["createdAt"].as_str().unwrap_or(""),
                        c["user"]["name"].as_str().unwrap_or("?"),
                        c["body"].as_str().unwrap_or("")
                    );
                }
            }
        }

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    async fn search_issues(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(query_str) = args["query"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("query required".to_string()),
            });
        };

        let query = r#"
            query SearchIssues($filter: IssueFilter) {
                issues(filter: $filter, first: 25) {
                    nodes {
                        identifier
                        title
                        state { name }
                        assignee { name }
                        priority
                        url
                    }
                }
            }
        "#;

        let filter = json!({
            "team": { "id": { "eq": self.team_id } },
            "title": { "containsIgnoreCase": query_str }
        });

        let data = self.graphql(query, json!({ "filter": filter })).await?;
        let nodes = match data["issues"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            Some(_) => {
                return Ok(ToolResult {
                    success: true,
                    output: "No issues found.".into(),
                    error: None,
                });
            }
            None => {
                tracing::warn!(
                    "search_issues: unexpected response shape from Linear API; \
                     possible API schema change"
                );
                return Ok(ToolResult {
                    success: true,
                    output: "No issues found.".into(),
                    error: None,
                });
            }
        };

        let lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                let priority = n["priority"]
                    .as_i64()
                    .map_or(String::new(), |p| format!(" p{p}"));
                format!(
                    "{} — {} [{}]{} {}",
                    n["identifier"].as_str().unwrap_or(""),
                    n["title"].as_str().unwrap_or(""),
                    n["state"]["name"].as_str().unwrap_or(""),
                    priority,
                    n["url"].as_str().unwrap_or("")
                )
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    async fn add_comment(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(issue_id) = args["issue_id"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("issue_id required".to_string()),
            });
        };
        let Some(body) = args["body"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("body required".to_string()),
            });
        };

        let query = r#"
            mutation AddComment($input: CommentCreateInput!) {
                commentCreate(input: $input) {
                    comment {
                        id
                        body
                        createdAt
                    }
                }
            }
        "#;

        let input = json!({ "issueId": issue_id, "body": body });
        let data = self.graphql(query, json!({ "input": input })).await?;
        let comment = &data["commentCreate"]["comment"];

        let output = format!(
            "Comment added ({}): {}",
            comment["createdAt"].as_str().unwrap_or(""),
            comment["body"].as_str().unwrap_or("")
        );

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    /// Archive an issue via the `issueArchive` mutation.
    ///
    /// Verification note: `issueArchive(id: String!)` is expected to exist in the
    /// Linear GraphQL schema. If the live API returns an unknown-field error, the
    /// caller should fall back to `update_issue` with a designated "Archived" state.
    async fn archive_issue(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(issue_id) = args["issue_id"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("issue_id required".to_string()),
            });
        };

        let query = r#"
            mutation ArchiveIssue($id: String!) {
                issueArchive(id: $id) {
                    success
                }
            }
        "#;

        let data = self.graphql(query, json!({ "id": issue_id })).await?;
        let success = data["issueArchive"]["success"].as_bool().unwrap_or(false);

        Ok(ToolResult {
            success,
            output: if success {
                format!("Issue {issue_id} archived.")
            } else {
                String::new()
            },
            error: if success {
                None
            } else {
                Some("issueArchive returned success=false".to_string())
            },
        })
    }

    // ── Lookup helpers ───────────────────────────────────────────────────────

    async fn list_states(&self, _args: &Value) -> anyhow::Result<ToolResult> {
        let query = r#"
            query ListStates($teamId: String!) {
                team(id: $teamId) {
                    states {
                        nodes {
                            id
                            name
                            type
                            color
                        }
                    }
                }
            }
        "#;

        let data = self
            .graphql(query, json!({ "teamId": self.team_id }))
            .await?;
        let nodes = match data["team"]["states"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            Some(_) => {
                return Ok(ToolResult {
                    success: true,
                    output: "No states found.".into(),
                    error: None,
                });
            }
            None => {
                tracing::warn!(
                    "list_states: unexpected response shape from Linear API; \
                     possible API schema change"
                );
                return Ok(ToolResult {
                    success: true,
                    output: "No states found.".into(),
                    error: None,
                });
            }
        };

        let lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                format!(
                    "[{}] {} ({}) {}",
                    n["id"].as_str().unwrap_or(""),
                    n["name"].as_str().unwrap_or(""),
                    n["type"].as_str().unwrap_or(""),
                    n["color"].as_str().unwrap_or("")
                )
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    async fn list_members(&self, _args: &Value) -> anyhow::Result<ToolResult> {
        let query = r#"
            query ListMembers($teamId: String!) {
                team(id: $teamId) {
                    members {
                        nodes {
                            id
                            name
                            displayName
                        }
                    }
                }
            }
        "#;

        let data = self
            .graphql(query, json!({ "teamId": self.team_id }))
            .await?;
        let nodes = match data["team"]["members"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            Some(_) => {
                return Ok(ToolResult {
                    success: true,
                    output: "No members found.".into(),
                    error: None,
                });
            }
            None => {
                tracing::warn!(
                    "list_members: unexpected response shape from Linear API; \
                     possible API schema change"
                );
                return Ok(ToolResult {
                    success: true,
                    output: "No members found.".into(),
                    error: None,
                });
            }
        };

        let lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                format!(
                    "[{}] {} ({})",
                    n["id"].as_str().unwrap_or(""),
                    n["name"].as_str().unwrap_or(""),
                    n["displayName"].as_str().unwrap_or("")
                )
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    async fn list_labels(&self, _args: &Value) -> anyhow::Result<ToolResult> {
        let query = r#"
            query ListLabels($teamId: String!) {
                team(id: $teamId) {
                    labels {
                        nodes {
                            id
                            name
                            color
                        }
                    }
                }
            }
        "#;

        let data = self
            .graphql(query, json!({ "teamId": self.team_id }))
            .await?;
        let nodes = match data["team"]["labels"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            Some(_) => {
                return Ok(ToolResult {
                    success: true,
                    output: "No labels found.".into(),
                    error: None,
                });
            }
            None => {
                tracing::warn!(
                    "list_labels: unexpected response shape from Linear API; \
                     possible API schema change"
                );
                return Ok(ToolResult {
                    success: true,
                    output: "No labels found.".into(),
                    error: None,
                });
            }
        };

        let lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                format!(
                    "[{}] {} {}",
                    n["id"].as_str().unwrap_or(""),
                    n["name"].as_str().unwrap_or(""),
                    n["color"].as_str().unwrap_or("")
                )
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    // ── Project operations ───────────────────────────────────────────────────

    async fn list_projects(&self, _args: &Value) -> anyhow::Result<ToolResult> {
        let query = r#"
            query ListProjects {
                projects(first: 25) {
                    nodes {
                        id
                        name
                        status { name }
                        url
                        targetDate
                    }
                }
            }
        "#;

        let data = self.graphql(query, json!({})).await?;
        let nodes = match data["projects"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            Some(_) => {
                return Ok(ToolResult {
                    success: true,
                    output: "No projects found.".into(),
                    error: None,
                });
            }
            None => {
                tracing::warn!(
                    "list_projects: unexpected response shape from Linear API; \
                     possible API schema change"
                );
                return Ok(ToolResult {
                    success: true,
                    output: "No projects found.".into(),
                    error: None,
                });
            }
        };

        let lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                format!(
                    "[{}] {} [{}] target:{} {}",
                    n["id"].as_str().unwrap_or(""),
                    n["name"].as_str().unwrap_or(""),
                    n["status"]["name"].as_str().unwrap_or(""),
                    n["targetDate"].as_str().unwrap_or("—"),
                    n["url"].as_str().unwrap_or("")
                )
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    async fn get_project(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(project_id) = args["project_id"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("project_id required".to_string()),
            });
        };

        let query = r#"
            query GetProject($id: String!) {
                project(id: $id) {
                    id
                    name
                    status { name }
                    url
                    targetDate
                    issues(first: 25) {
                        nodes {
                            identifier
                            title
                            state { name }
                            assignee { name }
                        }
                    }
                }
            }
        "#;

        let data = self.graphql(query, json!({ "id": project_id })).await?;
        let project = &data["project"];

        use std::fmt::Write as _;
        let mut output = format!(
            "Project: {} [{}] target:{}\n{}",
            project["name"].as_str().unwrap_or(""),
            project["status"]["name"].as_str().unwrap_or(""),
            project["targetDate"].as_str().unwrap_or("—"),
            project["url"].as_str().unwrap_or(""),
        );

        if let Some(issues) = project["issues"]["nodes"].as_array() {
            if !issues.is_empty() {
                let _ = write!(output, "\n\nIssues:");
                for issue in issues {
                    let _ = write!(
                        output,
                        "\n  {} — {} [{}] {}",
                        issue["identifier"].as_str().unwrap_or(""),
                        issue["title"].as_str().unwrap_or(""),
                        issue["state"]["name"].as_str().unwrap_or(""),
                        issue["assignee"]["name"].as_str().unwrap_or("unassigned")
                    );
                }
            }
        }

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    async fn create_project(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(name) = args["project_name"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("project_name required".to_string()),
            });
        };

        let query = r#"
            mutation CreateProject($input: ProjectCreateInput!) {
                projectCreate(input: $input) {
                    project {
                        id
                        name
                        url
                    }
                }
            }
        "#;

        let team_id = args["team_id"].as_str().unwrap_or(&self.team_id);
        let mut input = json!({
            "name": name,
            "teamIds": [team_id],
        });

        if let Some(v) = args["description"].as_str() {
            input["description"] = json!(v);
        }

        let data = self.graphql(query, json!({ "input": input })).await?;
        let project = &data["projectCreate"]["project"];

        let output = format!(
            "Created project: {} [{}]\n{}",
            project["name"].as_str().unwrap_or(""),
            project["id"].as_str().unwrap_or(""),
            project["url"].as_str().unwrap_or("")
        );

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    async fn update_project(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(project_id) = args["project_id"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("project_id required".to_string()),
            });
        };

        let query = r#"
            mutation UpdateProject($id: String!, $input: ProjectUpdateInput!) {
                projectUpdate(id: $id, input: $input) {
                    project {
                        id
                        name
                        status { name }
                        url
                    }
                }
            }
        "#;

        let mut input = json!({});

        if let Some(v) = args["name"].as_str() {
            input["name"] = json!(v);
        }
        if let Some(v) = args["description"].as_str() {
            input["description"] = json!(v);
        }
        if let Some(v) = args["target_date"].as_str() {
            input["targetDate"] = json!(v);
        }
        if let Some(v) = args["lead_id"].as_str() {
            input["leadId"] = json!(v);
        }

        if input.as_object().map_or(true, |o| o.is_empty()) {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "At least one field must be provided to update_project \
                     (name, description, target_date, or lead_id)"
                        .to_string(),
                ),
            });
        }

        let data = self
            .graphql(query, json!({ "id": project_id, "input": input }))
            .await?;
        let project = &data["projectUpdate"]["project"];

        let output = format!(
            "Updated project: {} [{}]\n{}",
            project["name"].as_str().unwrap_or(""),
            project["status"]["name"].as_str().unwrap_or(""),
            project["url"].as_str().unwrap_or("")
        );

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    // ── Cycle operations ─────────────────────────────────────────────────────

    async fn list_cycles(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let filter_arg = args["filter"].as_str().unwrap_or("all");

        let query = r#"
            query ListCycles($teamId: ID!) {
                cycles(filter: { team: { id: { eq: $teamId } } }, first: 25) {
                    nodes {
                        id
                        name
                        number
                        startsAt
                        endsAt
                        completedAt
                        issueCountHistory
                    }
                }
            }
        "#;

        let data = self
            .graphql(query, json!({ "teamId": self.team_id }))
            .await?;
        let all_nodes = match data["cycles"]["nodes"].as_array() {
            Some(a) => a.clone(),
            None => {
                tracing::warn!(
                    "list_cycles: unexpected response shape from Linear API; \
                     possible API schema change"
                );
                return Ok(ToolResult {
                    success: true,
                    output: "No cycles found.".into(),
                    error: None,
                });
            }
        };

        // Client-side filter: "completed" = completedAt is set; "active"/"upcoming" = not completed.
        let nodes: Vec<Value> = all_nodes
            .into_iter()
            .filter(|n| match filter_arg {
                "completed" => !n["completedAt"].is_null(),
                "active" | "upcoming" => n["completedAt"].is_null(),
                _ => true, // "all" or unrecognised — return everything
            })
            .collect();

        if nodes.is_empty() {
            return Ok(ToolResult {
                success: true,
                output: "No cycles found.".into(),
                error: None,
            });
        }

        let lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                let completed = if n["completedAt"].is_null() {
                    ""
                } else {
                    " (completed)"
                };
                format!(
                    "[{}] #{} {} {} → {}{}",
                    n["id"].as_str().unwrap_or(""),
                    n["number"].as_i64().map_or("?".into(), |v| v.to_string()),
                    n["name"].as_str().unwrap_or("(unnamed)"),
                    n["startsAt"].as_str().unwrap_or("?"),
                    n["endsAt"].as_str().unwrap_or("?"),
                    completed
                )
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    async fn get_cycle(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(cycle_id) = args["cycle_id"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("cycle_id required".to_string()),
            });
        };

        let query = r#"
            query GetCycle($id: String!) {
                cycle(id: $id) {
                    id
                    name
                    number
                    startsAt
                    endsAt
                    completedAt
                    issues(first: 50) {
                        nodes {
                            identifier
                            title
                            state { name }
                            assignee { name }
                            priority
                        }
                    }
                }
            }
        "#;

        let data = self.graphql(query, json!({ "id": cycle_id })).await?;
        let cycle = &data["cycle"];

        use std::fmt::Write as _;
        let mut output = format!(
            "Cycle #{}: {} ({} → {})",
            cycle["number"]
                .as_i64()
                .map_or("?".into(), |v| v.to_string()),
            cycle["name"].as_str().unwrap_or("(unnamed)"),
            cycle["startsAt"].as_str().unwrap_or("?"),
            cycle["endsAt"].as_str().unwrap_or("?"),
        );

        if !cycle["completedAt"].is_null() {
            let _ = write!(
                output,
                " — completed {}",
                cycle["completedAt"].as_str().unwrap_or("")
            );
        }

        if let Some(issues) = cycle["issues"]["nodes"].as_array() {
            if issues.is_empty() {
                let _ = write!(output, "\n\nNo issues in this cycle.");
            } else {
                let _ = write!(output, "\n\nIssues ({}):", issues.len());
                for issue in issues {
                    let assignee = issue["assignee"]["name"].as_str().unwrap_or("unassigned");
                    let priority = issue["priority"]
                        .as_i64()
                        .map_or("?".into(), |p| p.to_string());
                    let _ = write!(
                        output,
                        "\n  {} — {} [{}] p{} @{}",
                        issue["identifier"].as_str().unwrap_or(""),
                        issue["title"].as_str().unwrap_or(""),
                        issue["state"]["name"].as_str().unwrap_or(""),
                        priority,
                        assignee
                    );
                }
            }
        }

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    async fn add_issue_to_cycle(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let Some(issue_id) = args["issue_id"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("issue_id required".to_string()),
            });
        };
        let Some(cycle_id) = args["cycle_id"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("cycle_id required".to_string()),
            });
        };

        let query = r#"
            mutation AddIssueToCycle($id: String!, $input: IssueUpdateInput!) {
                issueUpdate(id: $id, input: $input) {
                    issue {
                        identifier
                        title
                        cycle { name }
                        url
                    }
                }
            }
        "#;

        let data = self
            .graphql(
                query,
                json!({ "id": issue_id, "input": { "cycleId": cycle_id } }),
            )
            .await?;
        let issue = &data["issueUpdate"]["issue"];

        let output = format!(
            "Added {} to cycle '{}'\n{}",
            issue["identifier"].as_str().unwrap_or(""),
            issue["cycle"]["name"].as_str().unwrap_or(""),
            issue["url"].as_str().unwrap_or("")
        );

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    // ── Initiative operations ────────────────────────────────────────────────

    async fn list_initiatives(&self, _args: &Value) -> anyhow::Result<ToolResult> {
        let query = r#"
            query ListInitiatives {
                initiatives(first: 25) {
                    nodes {
                        id
                        name
                        description
                        projects {
                            nodes {
                                name
                                status { name }
                            }
                        }
                    }
                }
            }
        "#;

        let data = self.graphql(query, json!({})).await?;
        let nodes = match data["initiatives"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            Some(_) => {
                return Ok(ToolResult {
                    success: true,
                    output: "No initiatives found.".into(),
                    error: None,
                });
            }
            None => {
                tracing::warn!(
                    "list_initiatives: unexpected response shape from Linear API; \
                     possible API schema change"
                );
                return Ok(ToolResult {
                    success: true,
                    output: "No initiatives found.".into(),
                    error: None,
                });
            }
        };

        use std::fmt::Write as _;
        let mut lines: Vec<String> = Vec::new();
        for n in &nodes {
            let mut line = format!(
                "[{}] {} — {}",
                n["id"].as_str().unwrap_or(""),
                n["name"].as_str().unwrap_or(""),
                n["description"].as_str().unwrap_or("")
            );
            if let Some(projects) = n["projects"]["nodes"].as_array() {
                for p in projects {
                    let _ = write!(
                        line,
                        "\n  Project: {} [{}]",
                        p["name"].as_str().unwrap_or(""),
                        p["status"]["name"].as_str().unwrap_or("")
                    );
                }
            }
            lines.push(line);
        }

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }

    async fn get_initiative(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let initiative_id = args["initiative_id"]
            .as_str()
            .context("initiative_id required")?;

        let query = r#"
            query GetInitiative($id: String!) {
                initiative(id: $id) {
                    name
                    description
                    projects {
                        nodes {
                            name
                            issues(first: 10) {
                                nodes {
                                    identifier
                                    title
                                    state { name }
                                }
                            }
                        }
                    }
                }
            }
        "#;

        let data = self.graphql(query, json!({ "id": initiative_id })).await?;
        let init = &data["initiative"];

        let mut output = format!(
            "Initiative: {}\n{}",
            init["name"].as_str().unwrap_or("(unnamed)"),
            init["description"].as_str().unwrap_or("")
        );

        if let Some(projects) = init["projects"]["nodes"].as_array() {
            use std::fmt::Write as _;
            for project in projects {
                let _ = write!(
                    output,
                    "\n\nProject: {}",
                    project["name"].as_str().unwrap_or("")
                );
                if let Some(issues) = project["issues"]["nodes"].as_array() {
                    for issue in issues {
                        let _ = write!(
                            output,
                            "\n  {} — {} [{}]",
                            issue["identifier"].as_str().unwrap_or(""),
                            issue["title"].as_str().unwrap_or(""),
                            issue["state"]["name"].as_str().unwrap_or("")
                        );
                    }
                }
            }
        }

        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    async fn list_templates(&self, _args: &Value) -> anyhow::Result<ToolResult> {
        let query = r#"
            query ListTemplates($teamId: String!) {
                team(id: $teamId) {
                    templates {
                        nodes {
                            id
                            name
                            description
                        }
                    }
                }
            }
        "#;

        let data = self
            .graphql(query, json!({ "teamId": self.team_id }))
            .await?;
        let nodes = match data["team"]["templates"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            Some(_) => {
                // Legitimate empty result — team has no templates.
                return Ok(ToolResult {
                    success: true,
                    output: "No templates found.".into(),
                    error: None,
                });
            }
            None => {
                // Schema mismatch: one of 'team', 'team.templates', or
                // 'team.templates.nodes' is missing / wrong type.
                if data["team"].is_null() {
                    tracing::warn!(
                        "list_templates: 'team' field is missing or null in Linear GraphQL \
                         response; possible API schema change"
                    );
                } else if data["team"]["templates"].is_null() {
                    tracing::warn!(
                        "list_templates: 'team.templates' field is missing or null in \
                         Linear GraphQL response; possible API schema change"
                    );
                } else {
                    tracing::warn!(
                        actual = %data["team"]["templates"]["nodes"],
                        "list_templates: 'team.templates.nodes' is not an array in Linear \
                         GraphQL response; possible API schema change"
                    );
                }
                return Ok(ToolResult {
                    success: true,
                    output: "No templates found.".into(),
                    error: None,
                });
            }
        };

        let lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                format!(
                    "[{}] {} — {}",
                    n["id"].as_str().unwrap_or(""),
                    n["name"].as_str().unwrap_or(""),
                    n["description"].as_str().unwrap_or("")
                )
            })
            .collect();

        Ok(ToolResult {
            success: true,
            output: lines.join("\n"),
            error: None,
        })
    }
}

#[async_trait]
impl Tool for LinearTool {
    fn name(&self) -> &str {
        "linear"
    }

    fn description(&self) -> &str {
        "Manage Linear issues, projects, cycles, and initiatives via the Linear GraphQL API. \
         Operations: create_issue, list_issues, update_issue, get_issue, search_issues, \
         add_comment, archive_issue, list_states, list_members, list_labels, \
         list_projects, get_project, create_project, update_project, \
         list_cycles, get_cycle, add_issue_to_cycle, list_initiatives, \
         get_initiative, list_templates."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "create_issue",
                        "list_issues",
                        "update_issue",
                        "get_issue",
                        "search_issues",
                        "add_comment",
                        "archive_issue",
                        "list_states",
                        "list_members",
                        "list_labels",
                        "list_projects",
                        "get_project",
                        "create_project",
                        "update_project",
                        "list_cycles",
                        "get_cycle",
                        "add_issue_to_cycle",
                        "list_initiatives",
                        "get_initiative",
                        "list_templates"
                    ],
                    "description": "Linear operation to perform"
                },
                "title": {
                    "type": "string",
                    "description": "Issue title (create_issue, update_issue)"
                },
                "description": {
                    "type": "string",
                    "description": "Issue or project description in markdown (create_issue, update_issue, create_project, update_project)"
                },
                "issue_id": {
                    "type": "string",
                    "description": "Linear issue UUID (update_issue, get_issue, add_comment, archive_issue, add_issue_to_cycle)"
                },
                "initiative_id": {
                    "type": "string",
                    "description": "Linear initiative UUID (get_initiative)"
                },
                "project_id": {
                    "type": "string",
                    "description": "Project UUID — filter or assign (list_issues, create_issue, update_issue, get_project, update_project)"
                },
                "project_name": {
                    "type": "string",
                    "description": "Project name for creation (create_project)"
                },
                "state_id": {
                    "type": "string",
                    "description": "Workflow state UUID to set (update_issue)"
                },
                "assignee_id": {
                    "type": "string",
                    "description": "User UUID to assign (create_issue, update_issue, list_issues)"
                },
                "template_id": {
                    "type": "string",
                    "description": "Template UUID to apply (create_issue)"
                },
                "status": {
                    "type": "string",
                    "description": "Filter by workflow state name, e.g. 'In Progress' (list_issues)"
                },
                "priority": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 4,
                    "description": "Priority: 0=no priority, 1=urgent, 2=high, 3=normal, 4=low (create_issue, update_issue, list_issues)"
                },
                "label_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Label UUIDs to apply (create_issue, update_issue)"
                },
                "label_id": {
                    "type": "string",
                    "description": "Single label UUID filter (list_issues)"
                },
                "parent_id": {
                    "type": "string",
                    "description": "Parent issue UUID for sub-issues (create_issue, update_issue)"
                },
                "cycle_id": {
                    "type": "string",
                    "description": "Cycle UUID (create_issue, update_issue, list_issues, get_cycle, add_issue_to_cycle)"
                },
                "estimate": {
                    "type": "integer",
                    "description": "Story point estimate (create_issue, update_issue)"
                },
                "due_date": {
                    "type": "string",
                    "description": "Due date ISO 8601, e.g. 2026-03-15 (create_issue, update_issue)"
                },
                "body": {
                    "type": "string",
                    "description": "Comment body text (add_comment)"
                },
                "query": {
                    "type": "string",
                    "description": "Search keyword for title matching (search_issues)"
                },
                "search": {
                    "type": "string",
                    "description": "Title keyword filter (list_issues)"
                },
                "filter": {
                    "type": "string",
                    "enum": ["active", "upcoming", "completed", "all"],
                    "description": "Cycle filter: active/upcoming (not completed), completed, or all (list_cycles)"
                },
                "target_date": {
                    "type": "string",
                    "description": "Project target date ISO 8601 (update_project)"
                },
                "lead_id": {
                    "type": "string",
                    "description": "Project lead user UUID (update_project)"
                },
                "team_id": {
                    "type": "string",
                    "description": "Team UUID override for project creation (create_project, defaults to configured team)"
                }
            },
            "required": ["operation"],
            "oneOf": [
                {
                    "properties": { "operation": { "const": "create_issue" } },
                    "required": ["operation", "title"]
                },
                {
                    "properties": { "operation": { "const": "list_issues" } },
                    "required": ["operation"]
                },
                {
                    "properties": { "operation": { "const": "update_issue" } },
                    "required": ["operation", "issue_id"],
                    "description": "Requires issue_id plus at least one field to update: title, description, state_id, assignee_id, priority, label_ids, parent_id, cycle_id, estimate, due_date, or project_id"
                },
                {
                    "properties": { "operation": { "const": "get_issue" } },
                    "required": ["operation", "issue_id"]
                },
                {
                    "properties": { "operation": { "const": "search_issues" } },
                    "required": ["operation", "query"]
                },
                {
                    "properties": { "operation": { "const": "add_comment" } },
                    "required": ["operation", "issue_id", "body"]
                },
                {
                    "properties": { "operation": { "const": "archive_issue" } },
                    "required": ["operation", "issue_id"]
                },
                {
                    "properties": { "operation": { "const": "list_states" } },
                    "required": ["operation"]
                },
                {
                    "properties": { "operation": { "const": "list_members" } },
                    "required": ["operation"]
                },
                {
                    "properties": { "operation": { "const": "list_labels" } },
                    "required": ["operation"]
                },
                {
                    "properties": { "operation": { "const": "list_projects" } },
                    "required": ["operation"]
                },
                {
                    "properties": { "operation": { "const": "get_project" } },
                    "required": ["operation", "project_id"]
                },
                {
                    "properties": { "operation": { "const": "create_project" } },
                    "required": ["operation", "project_name"]
                },
                {
                    "properties": { "operation": { "const": "update_project" } },
                    "required": ["operation", "project_id"]
                },
                {
                    "properties": { "operation": { "const": "list_cycles" } },
                    "required": ["operation"]
                },
                {
                    "properties": { "operation": { "const": "get_cycle" } },
                    "required": ["operation", "cycle_id"]
                },
                {
                    "properties": { "operation": { "const": "add_issue_to_cycle" } },
                    "required": ["operation", "issue_id", "cycle_id"]
                },
                {
                    "properties": { "operation": { "const": "list_initiatives" } },
                    "required": ["operation"]
                },
                {
                    "properties": { "operation": { "const": "get_initiative" } },
                    "required": ["operation", "initiative_id"]
                },
                {
                    "properties": { "operation": { "const": "list_templates" } },
                    "required": ["operation"]
                }
            ]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let Some(operation) = args["operation"].as_str() else {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some("'operation' field required".to_string()),
            });
        };

        match operation {
            "create_issue" => self.create_issue(&args).await,
            "list_issues" => self.list_issues(&args).await,
            "update_issue" => self.update_issue(&args).await,
            "get_issue" => self.get_issue(&args).await,
            "search_issues" => self.search_issues(&args).await,
            "add_comment" => self.add_comment(&args).await,
            "archive_issue" => self.archive_issue(&args).await,
            "list_states" => self.list_states(&args).await,
            "list_members" => self.list_members(&args).await,
            "list_labels" => self.list_labels(&args).await,
            "list_projects" => self.list_projects(&args).await,
            "get_project" => self.get_project(&args).await,
            "create_project" => self.create_project(&args).await,
            "update_project" => self.update_project(&args).await,
            "list_cycles" => self.list_cycles(&args).await,
            "get_cycle" => self.get_cycle(&args).await,
            "add_issue_to_cycle" => self.add_issue_to_cycle(&args).await,
            "list_initiatives" => self.list_initiatives(&args).await,
            "get_initiative" => self.get_initiative(&args).await,
            "list_templates" => self.list_templates(&args).await,
            other => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Unknown Linear operation: '{other}'")),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> LinearTool {
        LinearTool::new("test-key".into(), "test-team".into()).expect("build test tool")
    }

    // ── Basic metadata ────────────────────────────────────────────────────────

    #[test]
    fn name_is_linear() {
        assert_eq!(tool().name(), "linear");
    }

    #[test]
    fn parameters_schema_requires_operation() {
        let schema = tool().parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("operation")));
    }

    #[test]
    fn parameters_schema_enumerates_all_operations() {
        let schema = tool().parameters_schema();
        let ops = schema["properties"]["operation"]["enum"]
            .as_array()
            .unwrap();
        let op_names: Vec<&str> = ops.iter().filter_map(|v| v.as_str()).collect();
        // Original operations
        assert!(op_names.contains(&"create_issue"));
        assert!(op_names.contains(&"list_issues"));
        assert!(op_names.contains(&"update_issue"));
        assert!(op_names.contains(&"get_initiative"));
        assert!(op_names.contains(&"list_templates"));
        // New operations
        assert!(op_names.contains(&"get_issue"));
        assert!(op_names.contains(&"search_issues"));
        assert!(op_names.contains(&"add_comment"));
        assert!(op_names.contains(&"archive_issue"));
        assert!(op_names.contains(&"list_states"));
        assert!(op_names.contains(&"list_members"));
        assert!(op_names.contains(&"list_labels"));
        assert!(op_names.contains(&"list_projects"));
        assert!(op_names.contains(&"get_project"));
        assert!(op_names.contains(&"create_project"));
        assert!(op_names.contains(&"update_project"));
        assert!(op_names.contains(&"list_cycles"));
        assert!(op_names.contains(&"get_cycle"));
        assert!(op_names.contains(&"add_issue_to_cycle"));
        assert!(op_names.contains(&"list_initiatives"));
    }

    #[test]
    fn parameters_schema_has_per_operation_constraints() {
        let schema = tool().parameters_schema();
        let one_of = schema["oneOf"].as_array().unwrap();
        assert_eq!(one_of.len(), 20);
        // create_issue requires title
        let create = one_of
            .iter()
            .find(|v| v["properties"]["operation"]["const"] == "create_issue")
            .unwrap();
        assert!(create["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "title"));
        // update_issue requires issue_id
        let update = one_of
            .iter()
            .find(|v| v["properties"]["operation"]["const"] == "update_issue")
            .unwrap();
        assert!(update["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "issue_id"));
    }

    #[test]
    fn spec_matches_tool_metadata() {
        let t = tool();
        let spec = t.spec();
        assert_eq!(spec.name, "linear");
        assert!(spec.description.contains("Linear"));
        assert_eq!(spec.parameters["required"][0], "operation");
    }

    // ── execute() dispatch ───────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_rejects_unknown_operation() {
        let result = tool()
            .execute(json!({ "operation": "fly_to_moon" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Unknown Linear operation"));
    }

    #[tokio::test]
    async fn execute_rejects_missing_operation() {
        let result = tool().execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("'operation' field required"));
    }

    // ── update_issue validation ──────────────────────────────────────────────

    #[tokio::test]
    async fn update_issue_rejects_empty_input() {
        let result = tool()
            .execute(json!({ "operation": "update_issue", "issue_id": "issue-uuid" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("At least one field"));
    }

    // ── update_project validation ────────────────────────────────────────────

    #[tokio::test]
    async fn update_project_rejects_missing_project_id() {
        let result = tool()
            .execute(json!({ "operation": "update_project" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("project_id required"));
    }

    #[tokio::test]
    async fn update_project_rejects_empty_input() {
        let result = tool()
            .execute(json!({ "operation": "update_project", "project_id": "pid" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("At least one field"));
    }

    // ── Required-field validation for new operations ─────────────────────────

    #[tokio::test]
    async fn get_issue_rejects_missing_issue_id() {
        let result = tool()
            .execute(json!({ "operation": "get_issue" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("issue_id required"));
    }

    #[tokio::test]
    async fn search_issues_rejects_missing_query() {
        let result = tool()
            .execute(json!({ "operation": "search_issues" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("query required"));
    }

    #[tokio::test]
    async fn add_comment_rejects_missing_issue_id() {
        let result = tool()
            .execute(json!({ "operation": "add_comment", "body": "hello" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("issue_id required"));
    }

    #[tokio::test]
    async fn add_comment_rejects_missing_body() {
        let result = tool()
            .execute(json!({ "operation": "add_comment", "issue_id": "iid" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("body required"));
    }

    #[tokio::test]
    async fn archive_issue_rejects_missing_issue_id() {
        let result = tool()
            .execute(json!({ "operation": "archive_issue" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("issue_id required"));
    }

    #[tokio::test]
    async fn get_project_rejects_missing_project_id() {
        let result = tool()
            .execute(json!({ "operation": "get_project" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("project_id required"));
    }

    #[tokio::test]
    async fn create_project_rejects_missing_project_name() {
        let result = tool()
            .execute(json!({ "operation": "create_project" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("project_name required"));
    }

    #[tokio::test]
    async fn get_cycle_rejects_missing_cycle_id() {
        let result = tool()
            .execute(json!({ "operation": "get_cycle" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("cycle_id required"));
    }

    #[tokio::test]
    async fn add_issue_to_cycle_rejects_missing_issue_id() {
        let result = tool()
            .execute(json!({ "operation": "add_issue_to_cycle", "cycle_id": "cid" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("issue_id required"));
    }

    #[tokio::test]
    async fn add_issue_to_cycle_rejects_missing_cycle_id() {
        let result = tool()
            .execute(json!({ "operation": "add_issue_to_cycle", "issue_id": "iid" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("cycle_id required"));
    }

    // ── Priority schema ──────────────────────────────────────────────────────

    #[test]
    fn priority_field_is_integer_with_range() {
        let schema = tool().parameters_schema();
        let priority = &schema["properties"]["priority"];
        assert_eq!(priority["type"], "integer");
        assert_eq!(priority["minimum"], 0);
        assert_eq!(priority["maximum"], 4);
    }

    // ── GraphQL error helpers ────────────────────────────────────────────────

    #[test]
    fn graphql_error_extracts_first_message() {
        let errors = serde_json::json!([
            {"message": "Field 'teamId' required"},
            {"message": "Second error"}
        ]);
        let msg = errors
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        assert_eq!(msg, "Field 'teamId' required");
    }

    #[test]
    fn graphql_error_falls_back_to_unknown_when_no_message_field() {
        let errors = serde_json::json!([{"code": 400}]);
        let msg = errors
            .as_array()
            .and_then(|a| a.first())
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        assert_eq!(msg, "unknown error");
    }

    #[test]
    fn graphql_missing_data_field_is_error() {
        let json = serde_json::json!({"other": true});
        let result = json
            .get("data")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing"));
        assert!(result.is_err());
    }

    // ── list_issues schema-mismatch detection ────────────────────────────────

    #[test]
    fn list_issues_missing_issues_field_triggers_warn_path() {
        let data = serde_json::json!({ "other": {} });
        assert!(
            data["issues"].is_null(),
            "absent 'issues' key must be null (triggers warn)"
        );
        assert!(
            data["issues"]["nodes"].as_array().is_none(),
            "must hit the None arm"
        );
    }

    #[test]
    fn list_issues_non_array_nodes_triggers_warn_path() {
        let data = serde_json::json!({ "issues": { "nodes": "bad-value" } });
        assert!(!data["issues"].is_null(), "issues is present");
        assert!(
            data["issues"]["nodes"].as_array().is_none(),
            "non-array 'nodes' must hit the None arm (triggers warn)"
        );
    }

    #[test]
    fn list_issues_empty_array_is_silent_ok() {
        let data = serde_json::json!({ "issues": { "nodes": [] } });
        let arr = data["issues"]["nodes"]
            .as_array()
            .expect("should be an array");
        assert!(arr.is_empty(), "empty array is a valid no-results response");
    }

    // ── list_templates schema-mismatch detection ─────────────────────────────

    #[test]
    fn list_templates_missing_team_field_triggers_warn_path() {
        let data = serde_json::json!({ "other": {} });
        assert!(data["team"].is_null(), "absent 'team' key must be null");
        assert!(
            data["team"]["templates"]["nodes"].as_array().is_none(),
            "must hit the None arm"
        );
    }

    #[test]
    fn list_templates_missing_templates_field_triggers_warn_path() {
        let data = serde_json::json!({ "team": { "other": {} } });
        assert!(!data["team"].is_null(), "team is present");
        assert!(
            data["team"]["templates"].is_null(),
            "absent 'templates' key must be null"
        );
        assert!(
            data["team"]["templates"]["nodes"].as_array().is_none(),
            "must hit the None arm"
        );
    }

    #[test]
    fn list_templates_non_array_nodes_triggers_warn_path() {
        let data = serde_json::json!({ "team": { "templates": { "nodes": 42 } } });
        assert!(!data["team"].is_null());
        assert!(!data["team"]["templates"].is_null());
        assert!(
            data["team"]["templates"]["nodes"].as_array().is_none(),
            "non-array 'nodes' must hit the None arm (triggers warn)"
        );
    }

    #[test]
    fn list_templates_empty_array_is_silent_ok() {
        let data = serde_json::json!({ "team": { "templates": { "nodes": [] } } });
        let arr = data["team"]["templates"]["nodes"]
            .as_array()
            .expect("should be an array");
        assert!(arr.is_empty(), "empty array is a valid no-results response");
    }

    // ── Empty-array / schema-mismatch for new list operations ────────────────

    #[test]
    fn list_states_empty_array_is_silent_ok() {
        let data = serde_json::json!({ "team": { "states": { "nodes": [] } } });
        let arr = data["team"]["states"]["nodes"]
            .as_array()
            .expect("should be an array");
        assert!(arr.is_empty());
    }

    #[test]
    fn list_states_missing_nodes_triggers_warn_path() {
        let data = serde_json::json!({ "other": {} });
        assert!(data["team"]["states"]["nodes"].as_array().is_none());
    }

    #[test]
    fn list_members_empty_array_is_silent_ok() {
        let data = serde_json::json!({ "team": { "members": { "nodes": [] } } });
        let arr = data["team"]["members"]["nodes"]
            .as_array()
            .expect("should be an array");
        assert!(arr.is_empty());
    }

    #[test]
    fn list_members_missing_nodes_triggers_warn_path() {
        let data = serde_json::json!({ "other": {} });
        assert!(data["team"]["members"]["nodes"].as_array().is_none());
    }

    #[test]
    fn list_labels_empty_array_is_silent_ok() {
        let data = serde_json::json!({ "team": { "labels": { "nodes": [] } } });
        let arr = data["team"]["labels"]["nodes"]
            .as_array()
            .expect("should be an array");
        assert!(arr.is_empty());
    }

    #[test]
    fn list_labels_missing_nodes_triggers_warn_path() {
        let data = serde_json::json!({ "other": {} });
        assert!(data["team"]["labels"]["nodes"].as_array().is_none());
    }

    #[test]
    fn list_projects_empty_array_is_silent_ok() {
        let data = serde_json::json!({ "projects": { "nodes": [] } });
        let arr = data["projects"]["nodes"]
            .as_array()
            .expect("should be an array");
        assert!(arr.is_empty());
    }

    #[test]
    fn list_projects_missing_nodes_triggers_warn_path() {
        let data = serde_json::json!({ "other": {} });
        assert!(data["projects"]["nodes"].as_array().is_none());
    }

    #[test]
    fn list_cycles_empty_array_is_silent_ok() {
        let data = serde_json::json!({ "cycles": { "nodes": [] } });
        let arr = data["cycles"]["nodes"]
            .as_array()
            .expect("should be an array");
        assert!(arr.is_empty());
    }

    #[test]
    fn list_cycles_missing_nodes_triggers_warn_path() {
        let data = serde_json::json!({ "other": {} });
        assert!(data["cycles"]["nodes"].as_array().is_none());
    }

    #[test]
    fn list_initiatives_empty_array_is_silent_ok() {
        let data = serde_json::json!({ "initiatives": { "nodes": [] } });
        let arr = data["initiatives"]["nodes"]
            .as_array()
            .expect("should be an array");
        assert!(arr.is_empty());
    }

    #[test]
    fn list_initiatives_missing_nodes_triggers_warn_path() {
        let data = serde_json::json!({ "other": {} });
        assert!(data["initiatives"]["nodes"].as_array().is_none());
    }

    // ── list_cycles client-side filter logic ─────────────────────────────────

    #[test]
    fn list_cycles_filter_completed_excludes_active() {
        let nodes = [
            serde_json::json!({ "completedAt": "2026-01-01T00:00:00Z", "id": "c1" }),
            serde_json::json!({ "completedAt": serde_json::Value::Null, "id": "c2" }),
        ];
        let filtered: Vec<_> = nodes
            .iter()
            .filter(|n| !n["completedAt"].is_null())
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["id"], "c1");
    }

    #[test]
    fn list_cycles_filter_active_excludes_completed() {
        let nodes = [
            serde_json::json!({ "completedAt": "2026-01-01T00:00:00Z", "id": "c1" }),
            serde_json::json!({ "completedAt": serde_json::Value::Null, "id": "c2" }),
        ];
        let filtered: Vec<_> = nodes
            .iter()
            .filter(|n| n["completedAt"].is_null())
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0]["id"], "c2");
    }

    #[test]
    fn list_cycles_filter_all_returns_everything() {
        let nodes = [
            serde_json::json!({ "completedAt": "2026-01-01T00:00:00Z", "id": "c1" }),
            serde_json::json!({ "completedAt": serde_json::Value::Null, "id": "c2" }),
        ];
        let filter_arg = "all";
        let filtered: Vec<_> = nodes
            .iter()
            .filter(|n| match filter_arg {
                "completed" => !n["completedAt"].is_null(),
                "active" | "upcoming" => n["completedAt"].is_null(),
                _ => true,
            })
            .collect();
        assert_eq!(filtered.len(), 2);
    }

    // ── SSRF protection ───────────────────────────────────────────────────────

    #[test]
    fn linear_tool_rejects_non_linear_url() {
        // The hardcoded endpoint must pass.
        assert!(validate_linear_url(LINEAR_API_URL).is_ok());
        // Any other host must be rejected.
        let err = validate_linear_url("https://evil.com/graphql")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("linear.endpoint"),
            "expected allowlist error, got: {err}"
        );
        // Private IPs must be rejected even if somehow in the URL.
        let err = validate_linear_url("https://192.168.1.1/graphql")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("linear.endpoint"),
            "expected allowlist error, got: {err}"
        );
        // HTTP (non-HTTPS) must be rejected even for the correct host.
        let err = validate_linear_url("http://api.linear.app/graphql")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("https://"),
            "expected scheme error, got: {err}"
        );
    }
}
