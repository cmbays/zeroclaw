use crate::agent::prompt::{PromptContext, PromptSection};

/// Injects a `## Response Policy` section into the system prompt.
///
/// Controls when the agent responds vs. stays silent, and what tone/style to use.
/// Designed for use with `SystemPromptBuilder::add_section()` in Agent-based mode
/// pipelines. The channel pipeline (W2) pre-builds mode system prompts and inlines
/// the policy text directly in `ModeRegistry::from_config()`.
pub struct ResponsePolicySection {
    policy: String,
}

impl ResponsePolicySection {
    pub fn new(policy: impl Into<String>) -> Self {
        Self {
            policy: policy.into(),
        }
    }
}

impl PromptSection for ResponsePolicySection {
    fn name(&self) -> &str {
        "response_policy"
    }

    fn build(&self, _ctx: &PromptContext<'_>) -> anyhow::Result<String> {
        Ok(format!("## Response Policy\n\n{}", self.policy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_ctx<'a>(
        tools: &'a [Box<dyn crate::tools::Tool>],
        skills: &'a [crate::skills::Skill],
    ) -> PromptContext<'a> {
        PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools,
            skills,
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
        }
    }

    #[test]
    fn name_is_response_policy() {
        let s = ResponsePolicySection::new("respond when mentioned");
        assert_eq!(s.name(), "response_policy");
    }

    #[test]
    fn build_includes_header_and_policy_text() {
        let tools: Vec<Box<dyn crate::tools::Tool>> = vec![];
        let skills = vec![];
        let ctx = make_ctx(&tools, &skills);
        let s = ResponsePolicySection::new("respond when work items discussed");
        let output = s.build(&ctx).unwrap();
        assert!(output.starts_with("## Response Policy\n\n"));
        assert!(output.contains("respond when work items discussed"));
    }

    #[test]
    fn build_multiline_policy_preserved() {
        let tools: Vec<Box<dyn crate::tools::Tool>> = vec![];
        let skills = vec![];
        let ctx = make_ctx(&tools, &skills);
        let policy = "- respond when @mentioned\n- stay silent for social chat";
        let s = ResponsePolicySection::new(policy);
        let output = s.build(&ctx).unwrap();
        assert!(output.contains("- respond when @mentioned"));
        assert!(output.contains("- stay silent for social chat"));
    }

    #[test]
    fn empty_policy_builds_without_error() {
        let tools: Vec<Box<dyn crate::tools::Tool>> = vec![];
        let skills = vec![];
        let ctx = make_ctx(&tools, &skills);
        let s = ResponsePolicySection::new("");
        let output = s.build(&ctx).unwrap();
        assert_eq!(output, "## Response Policy\n\n");
    }
}
