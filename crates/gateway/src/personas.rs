use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_PERSONA_ID: &str = "default";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct PersonaFiles {
    pub identity: String,
    pub soul: String,
    pub tools: String,
    pub agents: String,
}

pub(crate) fn is_valid_persona_id(persona_id: &str) -> bool {
    let id = persona_id;
    if id.is_empty() || id.len() > 64 {
        return false;
    }
    id.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

pub(crate) fn persona_dir(persona_id: &str) -> anyhow::Result<PathBuf> {
    if !is_valid_persona_id(persona_id) {
        anyhow::bail!("invalid persona_id");
    }
    Ok(moltis_config::personas_dir().join(persona_id))
}

fn read_optional_string(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

fn write_string(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

pub(crate) fn ensure_default_persona_seeded() -> anyhow::Result<()> {
    let dir = persona_dir(DEFAULT_PERSONA_ID)?;
    std::fs::create_dir_all(&dir)?;

    let identity_path = dir.join("IDENTITY.md");
    if !identity_path.exists() {
        write_string(
            &identity_path,
            "---\nname: moltis\n---\n\n# IDENTITY.md\n\nSeeded default persona identity.\n",
        )?;
    }

    let soul_path = dir.join("SOUL.md");
    if !soul_path.exists() {
        write_string(&soul_path, moltis_config::DEFAULT_SOUL)?;
    }

    let tools_path = dir.join("TOOLS.md");
    if !tools_path.exists() {
        write_string(
            &tools_path,
            "# TOOLS.md\n\nAdd tool usage guidance here.\n",
        )?;
    }

    let agents_path = dir.join("AGENTS.md");
    if !agents_path.exists() {
        write_string(
            &agents_path,
            "# AGENTS.md\n\nAdd agent dispatching/routing guidance here.\n",
        )?;
    }

    Ok(())
}

pub(crate) fn list_personas() -> anyhow::Result<Vec<String>> {
    ensure_default_persona_seeded()?;

    let dir = moltis_config::personas_dir();
    let mut out = Vec::new();
    out.push(DEFAULT_PERSONA_ID.to_string());

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let id = name.to_string_lossy().to_string();
            if id == DEFAULT_PERSONA_ID {
                continue;
            }
            if is_valid_persona_id(&id) {
                out.push(id);
            }
        }
    }

    out.sort();
    // Keep default first.
    if let Some(pos) = out.iter().position(|id| id == DEFAULT_PERSONA_ID) {
        out.remove(pos);
    }
    out.insert(0, DEFAULT_PERSONA_ID.to_string());
    Ok(out)
}

pub(crate) fn get_persona(persona_id: &str) -> anyhow::Result<PersonaFiles> {
    if persona_id == DEFAULT_PERSONA_ID {
        ensure_default_persona_seeded()?;
    }
    let dir = persona_dir(persona_id)?;
    Ok(PersonaFiles {
        identity: read_optional_string(&dir.join("IDENTITY.md"))?,
        soul: read_optional_string(&dir.join("SOUL.md"))?,
        tools: read_optional_string(&dir.join("TOOLS.md"))?,
        agents: read_optional_string(&dir.join("AGENTS.md"))?,
    })
}

pub(crate) fn save_persona(persona_id: &str, files: &PersonaFiles) -> anyhow::Result<()> {
    let dir = persona_dir(persona_id)?;
    std::fs::create_dir_all(&dir)?;

    write_string(&dir.join("IDENTITY.md"), &files.identity)?;
    write_string(&dir.join("SOUL.md"), &files.soul)?;
    write_string(&dir.join("TOOLS.md"), &files.tools)?;
    write_string(&dir.join("AGENTS.md"), &files.agents)?;
    Ok(())
}

pub(crate) fn delete_persona(persona_id: &str) -> anyhow::Result<()> {
    if persona_id == DEFAULT_PERSONA_ID {
        anyhow::bail!("cannot delete default persona");
    }
    let dir = persona_dir(persona_id)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

pub(crate) fn clone_persona(from_id: &str, to_id: &str) -> anyhow::Result<()> {
    if from_id == to_id {
        anyhow::bail!("source and destination persona_id must differ");
    }
    let files = get_persona(from_id)?;
    let dest_dir = persona_dir(to_id)?;
    if dest_dir.exists() {
        anyhow::bail!("destination persona already exists");
    }
    save_persona(to_id, &files)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persona_id_validation_matches_loader_rules() {
        assert!(is_valid_persona_id("default"));
        assert!(is_valid_persona_id("ops_1"));
        assert!(is_valid_persona_id("ops-1"));
        assert!(!is_valid_persona_id(""));
        assert!(!is_valid_persona_id(" has-space "));
        assert!(!is_valid_persona_id("a/b"));
        assert!(!is_valid_persona_id("../x"));
    }

    #[test]
    fn list_personas_seeds_default_persona() {
        let _guard = crate::test_support::TestDirsGuard::new();
        let personas = list_personas().expect("list personas");
        assert!(
            personas.iter().any(|id| id == DEFAULT_PERSONA_ID),
            "default persona must always exist"
        );

        let dir = persona_dir(DEFAULT_PERSONA_ID).expect("default dir");
        assert!(dir.exists(), "default persona dir should exist");
        for file in ["IDENTITY.md", "SOUL.md", "TOOLS.md", "AGENTS.md"] {
            assert!(
                dir.join(file).exists(),
                "default persona file should exist: {file}"
            );
        }
    }

    #[test]
    fn delete_persona_rejects_default() {
        let _guard = crate::test_support::TestDirsGuard::new();
        let err = delete_persona(DEFAULT_PERSONA_ID).unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("default"),
            "error should mention default persona"
        );
    }

    #[test]
    fn clone_persona_copies_files() {
        let _guard = crate::test_support::TestDirsGuard::new();

        save_persona(
            "ops",
            &PersonaFiles {
                identity: "id".into(),
                soul: "s".into(),
                tools: "t".into(),
                agents: "a".into(),
            },
        )
        .expect("save source persona");

        clone_persona("ops", "ops2").expect("clone");
        let got = get_persona("ops2").expect("get clone");
        assert_eq!(
            got,
            PersonaFiles {
                identity: "id".into(),
                soul: "s".into(),
                tools: "t".into(),
                agents: "a".into(),
            }
        );
    }

    #[test]
    fn clone_from_default_seeds_default_template() {
        let _guard = crate::test_support::TestDirsGuard::new();

        clone_persona("default", "p1").expect("clone default");
        let got = get_persona("p1").expect("get clone");
        assert!(
            got.soul.contains(moltis_config::DEFAULT_SOUL),
            "clone should copy default SOUL.md template"
        );
        assert!(
            got.identity.contains("name: moltis"),
            "clone should copy default IDENTITY.md template"
        );
    }
}
