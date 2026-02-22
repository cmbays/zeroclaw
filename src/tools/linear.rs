//! Linear integration tool — issue CRUD, initiative lookup, template listing.
//!
//! Implements the [`Tool`] trait so the agent loop can call Linear's GraphQL API
//! directly. All network I/O goes through a single [`LinearTool::graphql`] helper
//! that adds authentication and validates the HTTP response.

use super::traits::{Tool, ToolResult};
use anyhow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};

const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

/// Linear API tool — exposes issue CRUD and project queries to the agent.
pub struct LinearTool {
    api_key: String,
    team_id: String,
    client: reqwest::Client,
}

impl LinearTool {
    pub fn new(api_key: String, team_id: String) -> Self {
        Self {
            api_key,
            team_id,
            client: reqwest::Client::new(),
        }
    }

    /// Execute a GraphQL query/mutation against the Linear API.
    async fn graphql(&self, query: &str, variables: Value) -> anyhow::Result<Value> {
        let body = json!({ "query": query, "variables": variables });

        let resp = self
            .client
            .post(LINEAR_API_URL)
            .bearer_auth(&self.api_key)
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
            anyhow::bail!("Linear GraphQL errors: {errors}");
        }

        Ok(json["data"].clone())
    }

    async fn create_issue(&self, args: &Value) -> anyhow::Result<ToolResult> {
        let title = args["title"].as_str().context("title required")?;
        let description = args["description"].as_str().unwrap_or("");

        let query = r#"
            mutation CreateIssue($input: IssueCreateInput!) {
                issueCreate(input: $input) {
                    success
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

        let data = self.graphql(query, json!({ "filter": filter })).await?;
        let nodes = match data["issues"]["nodes"].as_array() {
            Some(a) if !a.is_empty() => a.clone(),
            _ => {
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
                format!(
                    "{} — {} [{}] {}",
                    n["identifier"].as_str().unwrap_or(""),
                    n["title"].as_str().unwrap_or(""),
                    n["state"]["name"].as_str().unwrap_or(""),
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
                    success
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
            _ => {
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
        "Manage Linear issues, projects, and initiatives via the Linear GraphQL API. \
         Operations: create_issue, list_issues, update_issue, get_initiative, list_templates."
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
                    "description": "Issue description in markdown (create_issue, update_issue)"
                },
                "issue_id": {
                    "type": "string",
                    "description": "Linear issue UUID (update_issue)"
                },
                "initiative_id": {
                    "type": "string",
                    "description": "Linear initiative UUID (get_initiative)"
                },
                "project_id": {
                    "type": "string",
                    "description": "Project UUID — filter or assign (list_issues, create_issue)"
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
                }
            },
            "required": ["operation"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let operation = args["operation"]
            .as_str()
            .context("'operation' field required")?;

        match operation {
            "create_issue" => self.create_issue(&args).await,
            "list_issues" => self.list_issues(&args).await,
            "update_issue" => self.update_issue(&args).await,
            "get_initiative" => self.get_initiative(&args).await,
            "list_templates" => self.list_templates(&args).await,
            other => anyhow::bail!("Unknown Linear operation: '{other}'"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> LinearTool {
        LinearTool::new("test-key".into(), "test-team".into())
    }

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
        assert!(op_names.contains(&"create_issue"));
        assert!(op_names.contains(&"list_issues"));
        assert!(op_names.contains(&"update_issue"));
        assert!(op_names.contains(&"get_initiative"));
        assert!(op_names.contains(&"list_templates"));
    }

    #[test]
    fn spec_matches_tool_metadata() {
        let t = tool();
        let spec = t.spec();
        assert_eq!(spec.name, "linear");
        assert!(spec.description.contains("Linear"));
        assert_eq!(spec.parameters["required"][0], "operation");
    }

    #[tokio::test]
    async fn execute_rejects_unknown_operation() {
        let err = tool()
            .execute(json!({ "operation": "fly_to_moon" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Unknown Linear operation"));
    }

    #[tokio::test]
    async fn execute_rejects_missing_operation() {
        let err = tool().execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("'operation' field required"));
    }
}
