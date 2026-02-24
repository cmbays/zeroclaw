use super::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::json;

/// Tool that asks the user a clarifying question instead of guessing.
///
/// The agent calls `ask_user(question: "...")`, which returns the question
/// as its output. Thread continuation picks up the user's reply as the next
/// message — no async waiting or state management needed.
pub struct AskUserTool;

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a clarifying question when you need more context. \
         Use this instead of guessing when a request is ambiguous."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The clarifying question to ask the user"
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let question_val = args
            .get("question")
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'question'"))?;

        let question = question_val
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("'question' must be a string, got: {question_val}"))?;

        let question = question.trim();

        if question.is_empty() {
            let msg = "'question' parameter must not be empty";
            return Ok(ToolResult {
                success: false,
                output: msg.into(),
                error: Some(msg.into()),
            });
        }

        Ok(ToolResult {
            success: true,
            output: question.to_string(),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ask_user_name() {
        assert_eq!(AskUserTool.name(), "ask_user");
    }

    #[test]
    fn ask_user_description_not_empty() {
        assert!(!AskUserTool.description().is_empty());
    }

    #[test]
    fn ask_user_schema_has_question() {
        let schema = AskUserTool.parameters_schema();
        assert!(schema["properties"]["question"].is_object());
        assert!(schema["required"]
            .as_array()
            .expect("schema required field should be an array")
            .contains(&json!("question")));
    }

    #[tokio::test]
    async fn ask_user_returns_question_as_output() {
        let result = AskUserTool
            .execute(json!({ "question": "Which project are you asking about?" }))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "Which project are you asking about?");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn ask_user_trims_whitespace_from_output() {
        let result = AskUserTool
            .execute(json!({ "question": "  Which project?  " }))
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "Which project?");
    }

    #[tokio::test]
    async fn ask_user_missing_question_returns_err() {
        let result = AskUserTool.execute(json!({})).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("question"));
    }

    #[tokio::test]
    async fn ask_user_non_string_question_returns_err() {
        let result = AskUserTool.execute(json!({ "question": 42 })).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("string"),
            "expected type mismatch error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn ask_user_null_question_returns_err() {
        let result = AskUserTool.execute(json!({ "question": null })).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ask_user_blank_question_returns_failure() {
        let result = AskUserTool
            .execute(json!({ "question": "   " }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("must not be empty"));
        assert!(
            !result.output.is_empty(),
            "output should mirror error for consistent ToolResult contract"
        );
    }

    #[tokio::test]
    async fn ask_user_empty_string_question_returns_failure() {
        let result = AskUserTool
            .execute(json!({ "question": "" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or("")
            .contains("must not be empty"));
    }
}
