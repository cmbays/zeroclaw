use crate::config::IdentityConfig;
use crate::identity;
use crate::skills::Skill;
use crate::tools::Tool;
use anyhow::Result;
use chrono::Local;
use std::fmt::Write;
use std::path::Path;

const BOOTSTRAP_MAX_CHARS: usize = 20_000;

pub struct PromptContext<'a> {
    pub workspace_dir: &'a Path,
    pub model_name: &'a str,
    pub tools: &'a [Box<dyn Tool>],
    pub skills: &'a [Skill],
    pub skills_prompt_mode: crate::config::SkillsPromptInjectionMode,
    pub identity_config: Option<&'a IdentityConfig>,
    pub dispatcher_instructions: &'a str,
    pub team_config: Option<&'a crate::config::TeamConfig>,
}

pub trait PromptSection: Send + Sync {
    fn name(&self) -> &str;
    fn build(&self, ctx: &PromptContext<'_>) -> Result<String>;
}

#[derive(Default)]
pub struct SystemPromptBuilder {
    sections: Vec<Box<dyn PromptSection>>,
}

impl SystemPromptBuilder {
    pub fn with_defaults() -> Self {
        Self {
            sections: vec![
                Box::new(IdentitySection),
                Box::new(ToolsSection),
                Box::new(SafetySection),
                Box::new(SkillsSection),
                Box::new(WorkspaceSection),
                Box::new(DateTimeSection),
                Box::new(RuntimeSection),
                Box::new(TeamSection),
            ],
        }
    }

    pub fn add_section(mut self, section: Box<dyn PromptSection>) -> Self {
        self.sections.push(section);
        self
    }

    pub fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let mut output = String::new();
        for section in &self.sections {
            let part = section.build(ctx)?;
            if part.trim().is_empty() {
                continue;
            }
            output.push_str(part.trim_end());
            output.push_str("\n\n");
        }
        Ok(output)
    }
}

pub struct IdentitySection;
pub struct ToolsSection;
pub struct SafetySection;
pub struct SkillsSection;
pub struct WorkspaceSection;
pub struct RuntimeSection;
pub struct DateTimeSection;
pub struct TeamSection;

impl PromptSection for IdentitySection {
    fn name(&self) -> &str {
        "identity"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let mut prompt = String::from("## Project Context\n\n");
        let mut has_aieos = false;
        if let Some(config) = ctx.identity_config {
            if identity::is_aieos_configured(config) {
                if let Ok(Some(aieos)) = identity::load_aieos_identity(config, ctx.workspace_dir) {
                    let rendered = identity::aieos_to_system_prompt(&aieos);
                    if !rendered.is_empty() {
                        prompt.push_str(&rendered);
                        prompt.push_str("\n\n");
                        has_aieos = true;
                    }
                }
            }
        }

        if !has_aieos {
            prompt.push_str(
                "The following workspace files define your identity, behavior, and context.\n\n",
            );
        }
        for file in [
            "AGENTS.md",
            "SOUL.md",
            "TOOLS.md",
            "IDENTITY.md",
            "USER.md",
            "HEARTBEAT.md",
            "BOOTSTRAP.md",
            "MEMORY.md",
        ] {
            inject_workspace_file(&mut prompt, ctx.workspace_dir, file);
        }

        Ok(prompt)
    }
}

impl PromptSection for ToolsSection {
    fn name(&self) -> &str {
        "tools"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let mut out = String::from("## Tools\n\n");
        for tool in ctx.tools {
            let _ = writeln!(
                out,
                "- **{}**: {}\n  Parameters: `{}`",
                tool.name(),
                tool.description(),
                tool.parameters_schema()
            );
        }
        if !ctx.dispatcher_instructions.is_empty() {
            out.push('\n');
            out.push_str(ctx.dispatcher_instructions);
        }
        Ok(out)
    }
}

impl PromptSection for SafetySection {
    fn name(&self) -> &str {
        "safety"
    }

    fn build(&self, _ctx: &PromptContext<'_>) -> Result<String> {
        Ok("## Safety\n\n- Do not exfiltrate private data.\n- Do not run destructive commands without asking.\n- Do not bypass oversight or approval mechanisms.\n- Prefer `trash` over `rm`.\n- When in doubt, ask before acting externally.".into())
    }
}

impl PromptSection for SkillsSection {
    fn name(&self) -> &str {
        "skills"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        Ok(crate::skills::skills_to_prompt_with_mode(
            ctx.skills,
            ctx.workspace_dir,
            ctx.skills_prompt_mode,
        ))
    }
}

impl PromptSection for WorkspaceSection {
    fn name(&self) -> &str {
        "workspace"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        Ok(format!(
            "## Workspace\n\nWorking directory: `{}`",
            ctx.workspace_dir.display()
        ))
    }
}

impl PromptSection for RuntimeSection {
    fn name(&self) -> &str {
        "runtime"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let host =
            hostname::get().map_or_else(|_| "unknown".into(), |h| h.to_string_lossy().to_string());
        Ok(format!(
            "## Runtime\n\nHost: {host} | OS: {} | Model: {}",
            std::env::consts::OS,
            ctx.model_name
        ))
    }
}

impl PromptSection for DateTimeSection {
    fn name(&self) -> &str {
        "datetime"
    }

    fn build(&self, _ctx: &PromptContext<'_>) -> Result<String> {
        let now = Local::now();
        Ok(format!(
            "## Current Date & Time\n\n{} ({})",
            now.format("%Y-%m-%d %H:%M:%S"),
            now.format("%Z")
        ))
    }
}

impl PromptSection for TeamSection {
    fn name(&self) -> &str {
        "team"
    }

    fn build(&self, ctx: &PromptContext<'_>) -> Result<String> {
        let Some(team) = ctx.team_config else {
            return Ok(String::new());
        };
        if team.bots.is_empty() {
            return Ok(String::new());
        }

        let mut out = String::from("## Your Team\n\n");
        out.push_str("You are part of a bot team on Mattermost. When a request falls outside your expertise or tool allowlist, @mention the appropriate teammate in the current thread rather than attempting it yourself.\n\n");
        out.push_str("### Team Members\n\n");
        // writeln! on String via fmt::Write is always Ok; `let _ =` suppresses the unused-result lint
        for bot in &team.bots {
            if bot.username.is_empty() {
                tracing::warn!(
                    "team.bots entry has empty username — skipping malformed entry (role: {:?})",
                    bot.role
                );
                continue;
            }
            let _ = writeln!(
                out,
                "- **@{}** ({}): {}",
                bot.username, bot.role, bot.description
            );
        }
        if let Some(human) = &team.human_username {
            if human.is_empty() {
                tracing::warn!(
                    "team.human_username is set but empty — skipping human operator entry"
                );
            } else {
                let _ = writeln!(
                    out,
                    "- **@{human}**: Human operator — @mention when you need clarification, approval, or when the request is outside the team's scope"
                );
            }
        }
        out.push_str("\n### Delegation Rules\n\n");
        out.push_str("- When a request falls outside your tool allowlist, @mention the appropriate team member in the current thread\n");
        out.push_str("- When delegating, provide enough context for the receiving bot to act without asking follow-up questions\n");
        out.push_str("- Do not attempt tasks outside your capabilities — delegate instead");
        Ok(out)
    }
}

fn inject_workspace_file(prompt: &mut String, workspace_dir: &Path, filename: &str) {
    let path = workspace_dir.join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return;
            }
            let _ = writeln!(prompt, "### {filename}\n");
            let truncated = if trimmed.chars().count() > BOOTSTRAP_MAX_CHARS {
                trimmed
                    .char_indices()
                    .nth(BOOTSTRAP_MAX_CHARS)
                    .map(|(idx, _)| &trimmed[..idx])
                    .unwrap_or(trimmed)
            } else {
                trimmed
            };
            prompt.push_str(truncated);
            if truncated.len() < trimmed.len() {
                let _ = writeln!(
                    prompt,
                    "\n\n[... truncated at {BOOTSTRAP_MAX_CHARS} chars — use `read` for full file]\n"
                );
            } else {
                prompt.push_str("\n\n");
            }
        }
        Err(_) => {
            let _ = writeln!(prompt, "### {filename}\n\n[File not found: {filename}]\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::traits::Tool;
    use async_trait::async_trait;

    struct TestTool;

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            "test_tool"
        }

        fn description(&self) -> &str {
            "tool desc"
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(
            &self,
            _args: serde_json::Value,
        ) -> anyhow::Result<crate::tools::ToolResult> {
            Ok(crate::tools::ToolResult {
                success: true,
                output: "ok".into(),
                error: None,
            })
        }
    }

    #[test]
    fn identity_section_with_aieos_includes_workspace_files() {
        let workspace =
            std::env::temp_dir().join(format!("zeroclaw_prompt_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            workspace.join("AGENTS.md"),
            "Always respond with: AGENTS_MD_LOADED",
        )
        .unwrap();

        let identity_config = crate::config::IdentityConfig {
            format: "aieos".into(),
            aieos_path: None,
            aieos_inline: Some(r#"{"identity":{"names":{"first":"Nova"}}}"#.into()),
        };

        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: &workspace,
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: Some(&identity_config),
            dispatcher_instructions: "",
            team_config: None,
        };

        let section = IdentitySection;
        let output = section.build(&ctx).unwrap();

        assert!(
            output.contains("Nova"),
            "AIEOS identity should be present in prompt"
        );
        assert!(
            output.contains("AGENTS_MD_LOADED"),
            "AGENTS.md content should be present even when AIEOS is configured"
        );

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn prompt_builder_assembles_sections() {
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(TestTool)];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "instr",
            team_config: None,
        };
        let prompt = SystemPromptBuilder::with_defaults().build(&ctx).unwrap();
        assert!(prompt.contains("## Tools"));
        assert!(prompt.contains("test_tool"));
        assert!(prompt.contains("instr"));
        assert!(
            !prompt.contains("## Your Team"),
            "team section must not appear when team_config is None"
        );
    }

    #[test]
    fn skills_section_includes_instructions_and_tools() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let skills = vec![crate::skills::Skill {
            name: "deploy".into(),
            description: "Release safely".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "release_checklist".into(),
                description: "Validate release readiness".into(),
                kind: "shell".into(),
                command: "echo ok".into(),
                args: std::collections::HashMap::new(),
            }],
            prompts: vec!["Run smoke tests before deploy.".into()],
            location: None,
        }];

        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &skills,
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            team_config: None,
        };

        let output = SkillsSection.build(&ctx).unwrap();
        assert!(output.contains("<available_skills>"));
        assert!(output.contains("<name>deploy</name>"));
        assert!(output.contains("<instruction>Run smoke tests before deploy.</instruction>"));
        assert!(output.contains("<name>release_checklist</name>"));
        assert!(output.contains("<kind>shell</kind>"));
    }

    #[test]
    fn skills_section_compact_mode_omits_instructions_and_tools() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let skills = vec![crate::skills::Skill {
            name: "deploy".into(),
            description: "Release safely".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "release_checklist".into(),
                description: "Validate release readiness".into(),
                kind: "shell".into(),
                command: "echo ok".into(),
                args: std::collections::HashMap::new(),
            }],
            prompts: vec!["Run smoke tests before deploy.".into()],
            location: Some(Path::new("/tmp/workspace/skills/deploy/SKILL.md").to_path_buf()),
        }];

        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp/workspace"),
            model_name: "test-model",
            tools: &tools,
            skills: &skills,
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Compact,
            identity_config: None,
            dispatcher_instructions: "",
            team_config: None,
        };

        let output = SkillsSection.build(&ctx).unwrap();
        assert!(output.contains("<available_skills>"));
        assert!(output.contains("<name>deploy</name>"));
        assert!(output.contains("<location>skills/deploy/SKILL.md</location>"));
        assert!(!output.contains("<instruction>Run smoke tests before deploy.</instruction>"));
        assert!(!output.contains("<tools>"));
    }

    #[test]
    fn datetime_section_includes_timestamp_and_timezone() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "instr",
            team_config: None,
        };

        let rendered = DateTimeSection.build(&ctx).unwrap();
        assert!(rendered.starts_with("## Current Date & Time\n\n"));

        let payload = rendered.trim_start_matches("## Current Date & Time\n\n");
        assert!(payload.chars().any(|c| c.is_ascii_digit()));
        assert!(payload.contains(" ("));
        assert!(payload.ends_with(')'));
    }

    #[test]
    fn prompt_builder_inlines_and_escapes_skills() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let skills = vec![crate::skills::Skill {
            name: "code<review>&".into(),
            description: "Review \"unsafe\" and 'risky' bits".into(),
            version: "1.0.0".into(),
            author: None,
            tags: vec![],
            tools: vec![crate::skills::SkillTool {
                name: "run\"linter\"".into(),
                description: "Run <lint> & report".into(),
                kind: "shell&exec".into(),
                command: "cargo clippy".into(),
                args: std::collections::HashMap::new(),
            }],
            prompts: vec!["Use <tool_call> and & keep output \"safe\"".into()],
            location: None,
        }];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp/workspace"),
            model_name: "test-model",
            tools: &tools,
            skills: &skills,
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            team_config: None,
        };

        let prompt = SystemPromptBuilder::with_defaults().build(&ctx).unwrap();

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>code&lt;review&gt;&amp;</name>"));
        assert!(prompt.contains(
            "<description>Review &quot;unsafe&quot; and &apos;risky&apos; bits</description>"
        ));
        assert!(prompt.contains("<name>run&quot;linter&quot;</name>"));
        assert!(prompt.contains("<description>Run &lt;lint&gt; &amp; report</description>"));
        assert!(prompt.contains("<kind>shell&amp;exec</kind>"));
        assert!(prompt.contains(
            "<instruction>Use &lt;tool_call&gt; and &amp; keep output &quot;safe&quot;</instruction>"
        ));
    }

    #[test]
    fn team_section_renders_bots_and_human() {
        let team = crate::config::TeamConfig {
            human_username: Some("operator".into()),
            bots: vec![
                crate::config::TeamBotEntry {
                    username: "sokka".into(),
                    role: "Dev".into(),
                    description: "Code and debugging".into(),
                },
                crate::config::TeamBotEntry {
                    username: "katara".into(),
                    role: "PM".into(),
                    description: "Linear issues".into(),
                },
            ],
        };
        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            team_config: Some(&team),
        };

        let output = TeamSection.build(&ctx).unwrap();
        assert!(output.contains("## Your Team"));
        assert!(output.contains("@sokka"));
        assert!(output.contains("Dev"));
        assert!(output.contains("Code and debugging"));
        assert!(output.contains("@katara"));
        assert!(output.contains("@operator"));
        assert!(output.contains("Delegation Rules"));
    }

    #[test]
    fn team_section_empty_when_no_config() {
        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            team_config: None,
        };

        let output = TeamSection.build(&ctx).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn team_section_empty_when_bots_list_is_empty() {
        let team = crate::config::TeamConfig {
            human_username: Some("operator".into()),
            bots: vec![],
        };
        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            team_config: Some(&team),
        };

        let output = TeamSection.build(&ctx).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn team_section_renders_bots_without_human() {
        let team = crate::config::TeamConfig {
            human_username: None,
            bots: vec![crate::config::TeamBotEntry {
                username: "toph".into(),
                role: "DevOps".into(),
                description: "Infrastructure".into(),
            }],
        };
        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            team_config: Some(&team),
        };

        let output = TeamSection.build(&ctx).unwrap();
        assert!(output.contains("@toph"));
        assert!(
            !output.contains("Human operator"),
            "human line must be absent when human_username is None"
        );
        assert!(
            !output.contains("@None"),
            "must not render @None for missing human"
        );
    }

    #[test]
    fn team_section_skips_empty_username_bots() {
        let team = crate::config::TeamConfig {
            human_username: None,
            bots: vec![
                crate::config::TeamBotEntry {
                    username: String::new(),
                    role: "Ghost".into(),
                    description: "Empty username".into(),
                },
                crate::config::TeamBotEntry {
                    username: "sokka".into(),
                    role: "Dev".into(),
                    description: "Code".into(),
                },
            ],
        };
        let tools: Vec<Box<dyn Tool>> = vec![];
        let ctx = PromptContext {
            workspace_dir: Path::new("/tmp"),
            model_name: "test-model",
            tools: &tools,
            skills: &[],
            skills_prompt_mode: crate::config::SkillsPromptInjectionMode::Full,
            identity_config: None,
            dispatcher_instructions: "",
            team_config: Some(&team),
        };

        let output = TeamSection.build(&ctx).unwrap();
        assert!(output.contains("@sokka"), "valid bot must be rendered");
        assert!(
            !output.contains("@** (Ghost)"),
            "malformed empty-username entry must be skipped"
        );
    }

    #[test]
    fn team_config_toml_roundtrip_with_bots() {
        let toml_str = r#"
human_username = "zeroclaw_user"

[[bots]]
username = "sokka"
role = "Dev"
description = "Code and debugging"

[[bots]]
username = "katara"
role = "PM"
description = "Linear issues"
"#;
        let parsed: crate::config::TeamConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.human_username.as_deref(), Some("zeroclaw_user"));
        assert_eq!(parsed.bots.len(), 2);
        assert_eq!(parsed.bots[0].username, "sokka");
        assert_eq!(parsed.bots[0].role, "Dev");
        assert_eq!(parsed.bots[1].username, "katara");

        let re_serialized = toml::to_string_pretty(&parsed).unwrap();
        let re_parsed: crate::config::TeamConfig = toml::from_str(&re_serialized).unwrap();
        assert_eq!(re_parsed.bots.len(), 2);
        assert_eq!(re_parsed.bots[0].username, "sokka");
    }
}
