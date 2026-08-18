//! Skills 加载器：读 SKILL.md 注入 Agent 系统提示词。
//! 对齐 Pi Agent 的 additionalSkillPaths 行为。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LoadedSkill {
    pub name: String,
    pub path: PathBuf,
    pub markdown: String,
}

pub struct SkillLoader {
    root: PathBuf,
}

impl SkillLoader {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self { root: root.as_ref().to_path_buf() }
    }

    /// 扫描 root 下每个含 SKILL.md 的子目录
    pub fn load_all(&self) -> Result<Vec<LoadedSkill>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.root).with_context(|| {
            format!("skills dir not readable: {}", self.root.display())
        })? {
            let entry = entry?;
            let dir = entry.path();
            let skill_file = dir.join("SKILL.md");
            if dir.is_dir() && skill_file.exists() {
                let markdown = std::fs::read_to_string(&skill_file)?;
                let name = entry.file_name().to_string_lossy().to_string();
                out.push(LoadedSkill { name, path: dir, markdown });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// 拼装成注入系统提示词的文本块
    pub fn system_prompt_block(&self) -> Result<String> {
        let skills = self.load_all()?;
        let mut buf = String::new();
        for s in &skills {
            buf.push_str(&format!(
                "\n---\n# Skill: {}\n{}\n",
                s.name, s.markdown
            ));
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_skill_from_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("demo-skill");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), "# demo").unwrap();
        let loader = SkillLoader::new(tmp.path());
        let block = loader.system_prompt_block().unwrap();
        assert!(block.contains("demo-skill"));
        assert!(block.contains("# demo"));
    }
}
