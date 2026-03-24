use std::path::{Path, PathBuf};

use tracing::info;

use crate::types::{SkillMetadata, SkillSource};

pub const SANDBOX_SKILLS_GUEST_ROOT: &str = "/moltis/data/skills";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsPromptRuntime {
    Host,
    Sandbox,
}

/// Generate the `<available_skills>` XML block for injection into the system prompt.
pub fn generate_skills_prompt(skills: &[SkillMetadata]) -> String {
    generate_skills_prompt_for_runtime(skills, SkillsPromptRuntime::Host)
}

pub fn generate_skills_prompt_for_runtime(
    skills: &[SkillMetadata],
    runtime: SkillsPromptRuntime,
) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut sorted: Vec<(String, u8, String, String, String)> = skills
        .iter()
        .filter_map(|skill| {
            let (path_display, source_display) = match resolve_prompt_path(skill, runtime) {
                Some(resolved) => resolved,
                None => return None,
            };
            let source_key = match skill.source.as_ref() {
                Some(SkillSource::Project) => 1,
                Some(SkillSource::Personal) => 2,
                Some(SkillSource::Registry) => 3,
                Some(SkillSource::Plugin) => 4,
                None => 0,
            };
            Some((
                skill.name.clone(),
                source_key,
                path_display,
                source_display,
                skill.description.clone(),
            ))
        })
        .collect();
    // Stable ordering is required for prompt stability (diff/cache/debug).
    // Sort by (name, source, path).
    sorted
        .sort_by(|a, b| (a.0.as_str(), a.1, a.2.as_str()).cmp(&(b.0.as_str(), b.1, b.2.as_str())));

    if sorted.is_empty() {
        return String::new();
    }

    let mut out = String::from("## 可用技能\n\n<available_skills>\n");
    for (name, _, path_display, source_display, description) in sorted {
        out.push_str(&format!(
            "<skill name=\"{}\" source=\"{}\" path=\"{}\">\n{}\n</skill>\n",
            name, source_display, path_display, description,
        ));
    }
    out.push_str("</available_skills>\n\n");
    out.push_str(
        "启用技能：阅读对应的 SKILL.md（或插件 .md）以获取完整说明，然后按其中步骤执行。\n\n",
    );
    out
}

fn resolve_prompt_path(
    skill: &SkillMetadata,
    runtime: SkillsPromptRuntime,
) -> Option<(String, String)> {
    match runtime {
        SkillsPromptRuntime::Host => {
            let is_plugin = skill.source.as_ref() == Some(&SkillSource::Plugin);
            let path_display = if is_plugin {
                skill.path.display().to_string()
            } else {
                skill.path.join("SKILL.md").display().to_string()
            };
            let source_display = if is_plugin {
                "plugin"
            } else {
                "skill"
            }
            .to_string();
            Some((path_display, source_display))
        },
        SkillsPromptRuntime::Sandbox => match skill.source.as_ref() {
            Some(SkillSource::Personal) => {
                let basename = skill_dir_basename(&skill.path);
                Some((
                    PathBuf::from(SANDBOX_SKILLS_GUEST_ROOT)
                        .join(basename)
                        .join("SKILL.md")
                        .display()
                        .to_string(),
                    "skill".to_string(),
                ))
            },
            Some(source @ (SkillSource::Project | SkillSource::Registry | SkillSource::Plugin)) => {
                info!(
                    event = "skills_prompt_entry_filtered",
                    reason_code = "skill_source_not_exposed_in_sandbox",
                    decision = "filter",
                    policy = "sandbox_skill_path_contract",
                    skill_name = %skill.name,
                    skill_source = ?source,
                    "filtering unavailable skill source from sandbox prompt"
                );
                None
            },
            None => {
                info!(
                    event = "skills_prompt_entry_filtered",
                    reason_code = "unknown_skill_source_for_prompt_path",
                    decision = "filter",
                    policy = "sandbox_skill_path_contract",
                    skill_name = %skill.name,
                    "filtering skill with unknown source from sandbox prompt"
                );
                None
            },
        },
    }
}

fn skill_dir_basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::types::SkillSource;

    #[test]
    fn test_empty_skills_produces_empty_string() {
        assert_eq!(generate_skills_prompt(&[]), "");
    }

    #[test]
    fn test_single_skill_prompt() {
        let skills = vec![SkillMetadata {
            name: "commit".into(),
            description: "Create git commits".into(),
            license: None,
            compatibility: None,
            allowed_tools: vec![],
            homepage: None,
            dockerfile: None,
            requires: Default::default(),
            path: PathBuf::from("/home/user/.moltis/skills/commit"),
            source: None,
        }];
        let prompt = generate_skills_prompt(&skills);
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("name=\"commit\""));
        assert!(prompt.contains("Create git commits"));
        assert!(prompt.contains("SKILL.md"));
        assert!(prompt.contains("</available_skills>"));
    }

    #[test]
    fn test_multiple_skills() {
        let skills = vec![
            SkillMetadata {
                name: "commit".into(),
                description: "Commits".into(),
                license: None,
                compatibility: None,
                allowed_tools: vec![],
                homepage: None,
                dockerfile: None,
                requires: Default::default(),
                path: PathBuf::from("/a"),
                source: None,
            },
            SkillMetadata {
                name: "review".into(),
                description: "Reviews".into(),
                license: None,
                compatibility: None,
                allowed_tools: vec![],
                homepage: None,
                dockerfile: None,
                requires: Default::default(),
                path: PathBuf::from("/b"),
                source: None,
            },
        ];
        let prompt = generate_skills_prompt(&skills);
        assert!(prompt.contains("name=\"commit\""));
        assert!(prompt.contains("name=\"review\""));
    }

    #[test]
    fn test_sandbox_runtime_projects_personal_skill_to_guest_path() {
        let skills = vec![SkillMetadata {
            name: "friendly-name".into(),
            description: "Commits".into(),
            license: None,
            compatibility: None,
            allowed_tools: vec![],
            homepage: None,
            dockerfile: None,
            requires: Default::default(),
            path: PathBuf::from("/host/data/skills/dir-basename"),
            source: Some(SkillSource::Personal),
        }];

        let prompt = generate_skills_prompt_for_runtime(&skills, SkillsPromptRuntime::Sandbox);

        assert!(prompt.contains("/moltis/data/skills/dir-basename/SKILL.md"));
        assert!(!prompt.contains("/host/data/skills/dir-basename/SKILL.md"));
    }

    #[test]
    fn test_sandbox_runtime_filters_unavailable_sources() {
        let skills = vec![
            SkillMetadata {
                name: "project-skill".into(),
                description: "Project".into(),
                license: None,
                compatibility: None,
                allowed_tools: vec![],
                homepage: None,
                dockerfile: None,
                requires: Default::default(),
                path: PathBuf::from("/workspace/.moltis/skills/project-skill"),
                source: Some(SkillSource::Project),
            },
            SkillMetadata {
                name: "plugin-skill".into(),
                description: "Plugin".into(),
                license: None,
                compatibility: None,
                allowed_tools: vec![],
                homepage: None,
                dockerfile: None,
                requires: Default::default(),
                path: PathBuf::from("/host/data/installed-plugins/plugin-skill/README.md"),
                source: Some(SkillSource::Plugin),
            },
        ];

        let prompt = generate_skills_prompt_for_runtime(&skills, SkillsPromptRuntime::Sandbox);

        assert_eq!(prompt, "");
    }
}
