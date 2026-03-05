use crate::types::SkillMetadata;

/// Generate the `<available_skills>` XML block for injection into the system prompt.
pub fn generate_skills_prompt(skills: &[SkillMetadata]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    use crate::types::SkillSource;

    let mut sorted: Vec<(String, u8, String, &SkillMetadata)> = skills
        .iter()
        .map(|s| {
            let source_key = match s.source.as_ref() {
                Some(SkillSource::Project) => 1,
                Some(SkillSource::Personal) => 2,
                Some(SkillSource::Registry) => 3,
                Some(SkillSource::Plugin) => 4,
                None => 0,
            };
            (s.name.clone(), source_key, s.path.display().to_string(), s)
        })
        .collect();
    // Stable ordering is required for prompt stability (diff/cache/debug).
    // Sort by (name, source, path).
    sorted.sort_by(|a, b| (a.0.as_str(), a.1, a.2.as_str()).cmp(&(b.0.as_str(), b.1, b.2.as_str())));

    let mut out = String::from("## 可用技能\n\n<available_skills>\n");
    for (_, _, _, skill) in sorted {
        let is_plugin = skill.source.as_ref() == Some(&SkillSource::Plugin);
        let path_display = if is_plugin {
            skill.path.display().to_string()
        } else {
            skill.path.join("SKILL.md").display().to_string()
        };
        out.push_str(&format!(
            "<skill name=\"{}\" source=\"{}\" path=\"{}\">\n{}\n</skill>\n",
            skill.name,
            if is_plugin {
                "plugin"
            } else {
                "skill"
            },
            path_display,
            skill.description,
        ));
    }
    out.push_str("</available_skills>\n\n");
    out.push_str(
        "启用技能：阅读对应的 SKILL.md（或插件 .md）以获取完整说明，然后按其中步骤执行。\n\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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
}
