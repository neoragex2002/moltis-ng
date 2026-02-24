use std::path::PathBuf;

pub(crate) fn read_user_md() -> anyhow::Result<String> {
    let path = moltis_config::user_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn save_user_md(content: &str) -> anyhow::Result<PathBuf> {
    let path = moltis_config::user_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_md_roundtrips() {
        let _guard = crate::test_support::TestDirsGuard::new();
        let path = save_user_md("---\nname: Neo\n---\n").expect("save");
        assert_eq!(path, moltis_config::user_path());
        let got = read_user_md().expect("read");
        assert_eq!(got, "---\nname: Neo\n---\n");
    }
}
