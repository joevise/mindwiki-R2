//! # mw-agent — L3 运行时适配层
//!
//! 嵌入 r2-core（AgentSession），把 wiki skills 注入系统提示词。

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use mw_wiki::SkillLoader;
use r2_core::{config::Config, AgentSession};

pub struct WikiAgent {
    pub skills_root: PathBuf,
    pub work_dir: PathBuf,
}

impl WikiAgent {
    pub fn new(skills_root: impl Into<PathBuf>, work_dir: impl Into<PathBuf>) -> Self {
        Self { skills_root: skills_root.into(), work_dir: work_dir.into() }
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

    /// 从环境变量构造 r2-core Config：
    ///   MW_LLM_PROVIDER (openai_compat | anthropic，默认 openai_compat)
    ///   MW_LLM_BASE_URL / MW_LLM_API_KEY / MW_LLM_MODEL
    fn build_config(&self) -> Result<Config> {
        let provider = std::env::var("MW_LLM_PROVIDER").unwrap_or_else(|_| "openai_compat".into());
        let base_url = std::env::var("MW_LLM_BASE_URL").ok();
        let api_key = std::env::var("MW_LLM_API_KEY")
            .map_err(|_| anyhow!("环境变量 MW_LLM_API_KEY 未设置"))?;
        let model = std::env::var("MW_LLM_MODEL").ok();

        let mut config = Config::default_config();
        config.model.provider = provider;
        match config.model.provider.as_str() {
            "anthropic" => {
                if let Some(u) = base_url {
                    config.model.anthropic.base_url = u;
                }
                config.model.anthropic.api_key = api_key;
                if let Some(m) = model {
                    config.model.anthropic.model = m;
                }
            }
            "openai_compat" => {
                if let Some(u) = base_url {
                    config.model.openai_compat.base_url = u;
                }
                config.model.openai_compat.api_key = api_key;
                if let Some(m) = model {
                    config.model.openai_compat.model = m;
                }
            }
            other => {
                return Err(anyhow!(
                    "非法 MW_LLM_PROVIDER: \"{other}\"，仅支持 \"openai_compat\" 或 \"anthropic\""
                ))
            }
        }

        config.agent.work_dir = self.work_dir.to_string_lossy().into_owned();
        config.agent.system_prompt = self.system_prompt()?;
        config.resolve_auto_budget();
        Ok(config)
    }

    /// 真实跑一轮：创建 AgentSession（skills 系统提示词 + work_dir），prompt 一次，返回回答
    pub async fn ask(&self, question: &str) -> Result<String> {
        let config = self.build_config()?;
        let mut session = AgentSession::new(config).map_err(|e| anyhow!(e))?;
        session.prompt(question).await.map_err(|e| anyhow!(e))
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
        let agent = WikiAgent::new(tmp.path(), tmp.path());
        let p = agent.system_prompt().unwrap();
        assert!(p.contains("Mind Wiki"));
        assert!(p.contains("wiki-init"));
    }

    #[test]
    fn build_config_reads_env() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("MW_LLM_API_KEY", "sk-test");
        std::env::set_var("MW_LLM_PROVIDER", "anthropic");
        std::env::set_var("MW_LLM_BASE_URL", "https://example.com/v1");
        std::env::set_var("MW_LLM_MODEL", "test-model");
        let agent = WikiAgent::new(tmp.path(), tmp.path());
        let cfg = agent.build_config().unwrap();
        assert_eq!(cfg.model.provider, "anthropic");
        assert_eq!(cfg.model.anthropic.api_key, "sk-test");
        assert_eq!(cfg.model.anthropic.base_url, "https://example.com/v1");
        assert_eq!(cfg.model.anthropic.model, "test-model");
        assert!(cfg.agent.system_prompt.contains("Mind Wiki"));
        std::env::remove_var("MW_LLM_API_KEY");
        std::env::remove_var("MW_LLM_PROVIDER");
        std::env::remove_var("MW_LLM_BASE_URL");
        std::env::remove_var("MW_LLM_MODEL");
    }

    #[test]
    fn build_config_uses_self_work_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let skills = tmp.path().join("skills");
        let work = tmp.path().join("session-work");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::env::set_var("MW_LLM_API_KEY", "sk-test");
        let agent = WikiAgent::new(&skills, &work);
        let cfg = agent.build_config().unwrap();
        assert_eq!(cfg.agent.work_dir, work.to_string_lossy());
        std::env::remove_var("MW_LLM_API_KEY");
    }

    #[test]
    fn build_config_requires_api_key() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::remove_var("MW_LLM_API_KEY");
        let agent = WikiAgent::new(tmp.path(), tmp.path());
        assert!(agent.build_config().is_err());
    }

    #[test]
    fn build_config_rejects_bad_provider() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("MW_LLM_API_KEY", "sk-test");
        std::env::set_var("MW_LLM_PROVIDER", "bogus");
        let agent = WikiAgent::new(tmp.path(), tmp.path());
        assert!(agent.build_config().is_err());
        std::env::remove_var("MW_LLM_API_KEY");
        std::env::remove_var("MW_LLM_PROVIDER");
    }
}
