pub mod thread_state;

use std::collections::HashMap;

use crate::config::{IdentityConfig, VisualIdentityConfig};

/// A fully resolved mode ready for use at runtime.
pub struct ModeDefinition {
    pub name: String,
    /// Pre-built system prompt (includes AIEOS persona + response policy).
    pub system_prompt: String,
    /// Visual identity override for outbound messages.
    pub visual_identity: Option<VisualIdentityConfig>,
    /// Tool allowlist (empty = all tools). Stored for future use in W4A.
    pub allowed_tools: Vec<String>,
}

/// Registry of configured modes, built once at startup.
pub struct ModeRegistry {
    modes: HashMap<String, ModeDefinition>,
}

impl ModeRegistry {
    /// Build the registry from config. Each mode gets a pre-built system prompt
    /// using `build_system_prompt_with_mode` with mode-specific identity and skills.
    ///
    /// `tool_instructions_suffix` is the XML tool-call protocol block appended for
    /// non-native-tool providers (empty string for native-tool providers).
    pub fn from_config(
        config: &crate::config::Config,
        tool_descs: &[(&str, &str)],
        base_skills: &[crate::skills::Skill],
        bootstrap_max_chars: Option<usize>,
        native_tools: bool,
        skills_prompt_mode: crate::config::SkillsPromptInjectionMode,
        tool_instructions_suffix: &str,
    ) -> anyhow::Result<Self> {
        let mut modes = HashMap::new();

        for (name, mode_config) in &config.modes {
            // Validate: warn if aieos_path is set but identity_format is not "aieos"
            if mode_config.aieos_path.is_some() && mode_config.identity_format != "aieos" {
                tracing::warn!(
                    mode = name.as_str(),
                    format = mode_config.identity_format.as_str(),
                    "Mode has aieos_path set but identity_format is not \"aieos\"; \
                     AIEOS file will be ignored. Set identity_format = \"aieos\" to use it."
                );
            }

            // Validate: if aieos format, check that the file exists
            if mode_config.identity_format == "aieos" {
                if let Some(ref path) = mode_config.aieos_path {
                    let full_path = config.workspace_dir.join(path);
                    if !full_path.exists() {
                        anyhow::bail!(
                            "Mode '{}': aieos_path '{}' does not exist (resolved to '{}')",
                            name,
                            path,
                            full_path.display()
                        );
                    }
                }
            }

            // Warn if tool allowlist is configured but not yet enforced
            if !mode_config.tools.is_empty() {
                tracing::warn!(
                    mode = name.as_str(),
                    "Mode has tool allowlist configured but enforcement is not yet \
                     implemented (planned for W4A). All tools remain accessible."
                );
            }

            let mode_identity = IdentityConfig {
                format: mode_config.identity_format.clone(),
                aieos_path: mode_config.aieos_path.clone(),
                aieos_inline: None,
            };

            // Load mode-specific skills from skills_dir, falling back to base skills
            let mode_skills: Vec<crate::skills::Skill> =
                if let Some(ref dir) = mode_config.skills_dir {
                    let skills_path = config.workspace_dir.join(dir);
                    if !skills_path.exists() {
                        tracing::warn!(
                            mode = name.as_str(),
                            path = %skills_path.display(),
                            "Mode skills_dir does not exist; using base skills only"
                        );
                    }
                    let mut skills = crate::skills::load_skills_from_directory(&skills_path);
                    skills.extend(base_skills.iter().cloned());
                    skills
                } else {
                    base_skills.to_vec()
                };

            let mut prompt = crate::channels::build_system_prompt_with_mode(
                &config.workspace_dir,
                config.default_model.as_deref().unwrap_or("unknown"),
                tool_descs,
                &mode_skills,
                Some(&mode_identity),
                bootstrap_max_chars,
                native_tools,
                skills_prompt_mode,
            );

            // Append tool instructions for non-native-tool providers
            if !tool_instructions_suffix.is_empty() {
                prompt.push_str(tool_instructions_suffix);
            }

            // Append response policy if configured
            if let Some(ref policy) = mode_config.response_policy {
                prompt.push_str("\n## Response Policy\n\n");
                prompt.push_str(policy);
                prompt.push_str("\n\n");
            }

            modes.insert(
                name.clone(),
                ModeDefinition {
                    name: name.clone(),
                    system_prompt: prompt,
                    visual_identity: mode_config.visual_identity.clone(),
                    allowed_tools: mode_config.tools.clone(),
                },
            );
        }

        Ok(Self { modes })
    }

    pub fn has_mode(&self, name: &str) -> bool {
        self.modes.contains_key(name)
    }

    pub fn get_mode(&self, name: &str) -> Option<&ModeDefinition> {
        self.modes.get(name)
    }

    pub fn mode_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.modes.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }
}

/// Maximum length for a mode name candidate (defense-in-depth against oversized allocations).
const MAX_MODE_NAME_LEN: usize = 64;

/// Parse mode activation from message text.
/// Matches: `<@UBOT> [pm]`, `<@UBOT> pm` (single word after mention).
/// Returns lowercase mode name or None.
pub fn parse_mode_activation(text: &str) -> Option<String> {
    let text = text.trim();

    // Look for Slack mention pattern: <@UXXXXXX>
    let mention_start = text.find("<@")?;
    let after_at = &text[mention_start + 2..];
    let close_bracket = after_at.find('>')?;
    let after_mention = after_at[close_bracket + 1..].trim();

    if after_mention.is_empty() {
        return None;
    }

    // Try bracketed: [pm]
    if after_mention.starts_with('[') {
        let end = after_mention.find(']')?;
        let mode = after_mention[1..end].trim();
        if !mode.is_empty() && !mode.contains(' ') && mode.len() <= MAX_MODE_NAME_LEN {
            return Some(mode.to_lowercase());
        }
    }

    // Try single word (no spaces, no brackets) - only if entire remaining text is a single word
    let first_word_end = after_mention.find(char::is_whitespace);
    let first_word = match first_word_end {
        Some(pos) => &after_mention[..pos],
        None => after_mention,
    };

    // Only activate for single-word messages (no additional text unless brackets were used)
    if first_word_end.is_none()
        && !first_word.contains('[')
        && first_word.len() <= MAX_MODE_NAME_LEN
    {
        return Some(first_word.to_lowercase());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_activation_with_brackets() {
        assert_eq!(
            parse_mode_activation("<@U1234> [pm]"),
            Some("pm".to_string())
        );
    }

    #[test]
    fn parse_mode_activation_without_brackets() {
        assert_eq!(parse_mode_activation("<@U1234> pm"), Some("pm".to_string()));
    }

    #[test]
    fn parse_mode_activation_case_insensitive() {
        assert_eq!(
            parse_mode_activation("<@U1234> [PM]"),
            Some("pm".to_string())
        );
    }

    #[test]
    fn parse_mode_activation_empty_after_mention() {
        assert_eq!(parse_mode_activation("<@U1234>"), None);
    }

    #[test]
    fn parse_mode_activation_no_mention() {
        assert_eq!(parse_mode_activation("pm"), None);
    }

    #[test]
    fn parse_mode_activation_multi_word() {
        assert_eq!(parse_mode_activation("<@U1234> create an issue"), None);
    }

    #[test]
    fn parse_mode_activation_with_extra_text_in_brackets() {
        // Bracketed mode should still match even with trailing text
        assert_eq!(
            parse_mode_activation("<@U1234> [pm] hello"),
            Some("pm".to_string())
        );
    }

    #[test]
    fn parse_mode_activation_empty_brackets() {
        assert_eq!(parse_mode_activation("<@U1234> []"), None);
    }

    #[test]
    fn parse_mode_activation_rejects_oversized_name() {
        let long_name = "a".repeat(65);
        let msg = format!("<@U1234> {}", long_name);
        assert_eq!(parse_mode_activation(&msg), None);

        let bracketed = format!("<@U1234> [{}]", long_name);
        assert_eq!(parse_mode_activation(&bracketed), None);
    }

    #[test]
    fn parse_mode_activation_accepts_max_length_name() {
        let name = "a".repeat(64);
        let msg = format!("<@U1234> {}", name);
        assert_eq!(parse_mode_activation(&msg), Some(name));
    }

    #[test]
    fn mode_registry_empty() {
        let config = crate::config::Config::default();
        let registry = ModeRegistry::from_config(
            &config,
            &[],
            &[],
            None,
            true,
            crate::config::SkillsPromptInjectionMode::Full,
            "",
        )
        .unwrap();
        assert!(registry.mode_names().is_empty());
        assert!(!registry.has_mode("pm"));
        assert!(registry.get_mode("pm").is_none());
    }
}
