//! # mw-agent — L3 运行时适配层
//!
//! 嵌入 r2-core（AgentSession），把 wiki skills 注入系统提示词。
//! Step1 先以占位接口落地，Step2 接通真实会话。

use anyhow::Result;
use mw_wiki::SkillLoader;

pub struct WikiAgent {
    pub skills_root: std::path::PathBuf,
}

impl WikiAgent {
    pub fn new(skills_root: impl Into<std::path::PathBuf>) -> Self {
        Self { skills_root: skills_root.into() }
    }

    /// 构造带 skills 的 Agent 系统提示词
    pub fn system_prompt(&self) -> Result<String> {
        let loader = SkillLoader::new(&self.skills_root);
        let mut prompt = String::from(
            "你是 Mind Wiki 的知识库引擎。严格遵守 skills 中的方法与铁律。\n",
        );
        prompt.push_str(&loader.system_prompt_block()?);
        Ok(prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_prompt_with_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("wiki-init");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), "# wiki init skill").unwrap();
        let agent = WikiAgent::new(tmp.path());
        let p = agent.system_prompt().unwrap();
        assert!(p.contains("Mind Wiki"));
        assert!(p.contains("wiki-init"));
    }
}
