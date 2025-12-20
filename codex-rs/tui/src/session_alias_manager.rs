use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
struct SessionAliasFile {
    aliases: HashMap<String, String>,
}

/// Stores user-defined session aliases on disk.
///
/// The aliases file lives at `<codex_home>/session_aliases.json`.
#[derive(Debug, Default, Clone)]
pub(crate) struct SessionAliasManager {
    codex_home: PathBuf,
    aliases: HashMap<String, String>,
}

impl SessionAliasManager {
    pub(crate) fn load(codex_home: PathBuf) -> Self {
        let path = codex_home.join("session_aliases.json");
        if let Ok(content) = fs::read_to_string(&path)
            && let Ok(file) = serde_json::from_str::<SessionAliasFile>(&content)
        {
            return Self {
                codex_home,
                aliases: file.aliases,
            };
        }

        Self {
            codex_home,
            aliases: HashMap::new(),
        }
    }

    pub(crate) fn set_alias(&mut self, session_id: String, alias: String) {
        let alias = alias.trim().to_string();
        if alias.is_empty() || alias.chars().count() > 30 {
            return;
        }

        self.aliases.insert(session_id, alias);
        let _ = self.save();
    }

    pub(crate) fn get_alias(&self, session_id: &str) -> Option<String> {
        self.aliases.get(session_id).cloned()
    }

    pub(crate) fn remove_alias(&mut self, session_id: &str) -> Option<String> {
        let removed = self.aliases.remove(session_id);
        if removed.is_some() {
            let _ = self.save();
        }
        removed
    }

    fn save(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.codex_home)?;
        let path = self.codex_home.join("session_aliases.json");
        let temp_path = path.with_extension("tmp");
        let content = serde_json::to_string_pretty(&SessionAliasFile {
            aliases: self.aliases.clone(),
        })?;
        fs::write(&temp_path, content)?;
        fs::rename(temp_path, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[test]
    fn round_trips_alias_file() {
        let tmp = TempDir::new().expect("tempdir");
        let codex_home = tmp.path().to_path_buf();

        let mut manager = SessionAliasManager::load(codex_home.clone());
        manager.set_alias("session1".to_string(), "测试会话".to_string());
        manager.set_alias("session2".to_string(), "  ".to_string());

        let loaded = SessionAliasManager::load(codex_home);
        assert_eq!(loaded.get_alias("session1"), Some("测试会话".to_string()));
        assert_eq!(loaded.get_alias("session2"), None);
    }
}
