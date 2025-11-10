//! 会话别名管理器
//! 
//! 负责管理会话的用户自定义别名，存储在 ~/.codex/session_aliases.json

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use serde::{Serialize, Deserialize};

/// 会话别名管理器
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct SessionAliasManager {
    /// 会话ID到别名的映射
    aliases: HashMap<String, String>,
}

impl SessionAliasManager {
    /// 从文件加载别名管理器
    pub fn load() -> Self {
        let path = Self::file_path();
        if let Ok(content) = fs::read_to_string(&path) {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }
    
    /// 保存别名管理器到文件
    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::file_path();
        
        // 确保目录存在
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        // 使用原子写入：先写临时文件，再重命名
        let temp_path = path.with_extension("tmp");
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&temp_path, content)?;
        fs::rename(temp_path, path)?;
        
        Ok(())
    }
    
    /// 设置会话别名
    pub fn set_alias(&mut self, session_id: String, alias: String) {
        let alias = alias.trim().to_string();
        if !alias.is_empty() && alias.len() <= 30 {  // 限制别名长度
            self.aliases.insert(session_id, alias);
            let _ = self.save();  // 忽略保存错误，不影响功能
        }
    }
    
    /// 获取会话别名
    pub fn get_alias(&self, session_id: &str) -> Option<String> {
        self.aliases.get(session_id).cloned()
    }
    
    /// 移除会话别名
    pub fn remove_alias(&mut self, session_id: &str) -> Option<String> {
        let removed = self.aliases.remove(session_id);
        if removed.is_some() {
            let _ = self.save();  // 忽略保存错误
        }
        removed
    }
    
    /// 检查是否有别名
    pub fn has_alias(&self, session_id: &str) -> bool {
        self.aliases.contains_key(session_id)
    }
    
    /// 获取所有别名（用于调试或导出）
    pub fn all_aliases(&self) -> &HashMap<String, String> {
        &self.aliases
    }
    
    /// 清空所有别名（谨慎使用）
    pub fn clear(&mut self) {
        self.aliases.clear();
        let _ = self.save();
    }
    
    /// 获取别名文件路径
    fn file_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codex")
            .join("session_aliases.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_alias_operations() {
        let mut manager = SessionAliasManager::default();
        
        // 测试设置和获取别名
        manager.set_alias("session1".to_string(), "测试会话".to_string());
        assert_eq!(manager.get_alias("session1"), Some("测试会话".to_string()));
        
        // 测试空白字符串不会被保存
        manager.set_alias("session2".to_string(), "  ".to_string());
        assert_eq!(manager.get_alias("session2"), None);
        
        // 测试移除别名
        manager.remove_alias("session1");
        assert_eq!(manager.get_alias("session1"), None);
    }
}
