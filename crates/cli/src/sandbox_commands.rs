use {anyhow::Result, clap::Subcommand};

use moltis_tools::sandbox;

fn sanitize_instance_slug(name: &str) -> String {
    let base = name.to_lowercase();
    let mut out = String::new();
    let mut last_dash = false;
    for ch in base.chars() {
        let mapped = if ch.is_ascii_alphanumeric() {
            ch
        } else {
            '-'
        };
        if mapped == '-' {
            if !last_dash {
                out.push(mapped);
            }
            last_dash = true;
        } else {
            out.push(mapped);
            last_dash = false;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "moltis".to_string()
    } else {
        out
    }
}

fn instance_sandbox_prefix(_config: &moltis_config::MoltisConfig) -> String {
    let display_name = load_default_display_name().unwrap_or_else(|| "moltis".to_string());
    let slug = sanitize_instance_slug(&display_name);
    format!("moltis-{slug}-sandbox")
}

fn load_default_display_name() -> Option<String> {
    let raw = std::fs::read_to_string(moltis_config::people_path()).ok()?;
    let frontmatter = extract_yaml_frontmatter(&raw)?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(frontmatter).ok()?;
    let people = yaml.get("people")?.as_sequence()?;
    for item in people {
        let entry = item.as_mapping()?;
        let name = entry
            .get(&serde_yaml::Value::String("name".to_string()))?
            .as_str()?
            .trim();
        if name == "default" {
            return entry
                .get(&serde_yaml::Value::String("display_name".to_string()))
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
    }
    None
}

fn extract_yaml_frontmatter(content: &str) -> Option<&str> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n')?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

#[derive(Subcommand)]
pub enum SandboxAction {
    /// List pre-built sandbox images.
    List,
    /// Build a sandbox image from the configured base + packages.
    Build,
    /// Remove a specific sandbox image by tag.
    Remove {
        /// Image tag (e.g. moltis-main-sandbox:abc123).
        tag: String,
    },
    /// Remove all pre-built sandbox images.
    Clean,
}

pub async fn handle_sandbox(action: SandboxAction) -> Result<()> {
    match action {
        SandboxAction::List => list().await,
        SandboxAction::Build => build().await,
        SandboxAction::Remove { tag } => remove(&tag).await,
        SandboxAction::Clean => clean().await,
    }
}

async fn list() -> Result<()> {
    let images = sandbox::list_sandbox_images().await?;
    if images.is_empty() {
        println!("No sandbox images found.");
        return Ok(());
    }
    println!("{:<45} {:>10}  CREATED", "TAG", "SIZE");
    for img in &images {
        println!("{:<45} {:>10}  {}", img.tag, img.size, img.created);
    }
    Ok(())
}

async fn build() -> Result<()> {
    let config = moltis_config::discover_and_load();
    let mut sandbox_config = sandbox::SandboxConfig::from(&config.tools.exec.sandbox);
    sandbox_config.container_prefix = Some(instance_sandbox_prefix(&config));

    let packages = sandbox_config.packages.clone();
    if packages.is_empty() {
        println!("No packages configured — nothing to build.");
        println!("Add packages to [tools.exec.sandbox] in your config file.");
        return Ok(());
    }

    let base = sandbox_config
        .image
        .clone()
        .unwrap_or_else(|| sandbox::DEFAULT_SANDBOX_IMAGE.to_string());
    let repo = sandbox_config
        .container_prefix
        .clone()
        .unwrap_or_else(|| "moltis-sandbox".to_string());
    let tag = sandbox::sandbox_image_tag(&repo, &base, &packages);
    println!("Base:     {base}");
    println!("Packages: {}", packages.join(", "));
    println!("Tag:      {tag}");
    println!();

    // Force mode to All so create_sandbox returns a real backend.
    let sandbox_config = sandbox::SandboxConfig {
        mode: sandbox::SandboxMode::All,
        ..sandbox_config
    };
    let backend = sandbox::create_sandbox(sandbox_config);
    match backend.build_image(&base, &packages).await? {
        Some(result) => {
            if result.built {
                println!("Image built successfully: {}", result.tag);
            } else {
                println!("Image already exists: {}", result.tag);
            }
        },
        None => {
            println!(
                "Backend '{}' does not support image building.",
                backend.backend_name()
            );
        },
    }
    Ok(())
}

async fn remove(tag: &str) -> Result<()> {
    sandbox::remove_sandbox_image(tag).await?;
    println!("Removed: {tag}");
    Ok(())
}

async fn clean() -> Result<()> {
    let count = sandbox::clean_sandbox_images().await?;
    if count == 0 {
        println!("No sandbox images to remove.");
    } else {
        println!(
            "Removed {count} sandbox image{}.",
            if count == 1 {
                ""
            } else {
                "s"
            }
        );
    }
    Ok(())
}
