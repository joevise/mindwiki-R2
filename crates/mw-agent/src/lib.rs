//! # mw-agent — L3 运行时适配层
//!
//! 嵌入 r2-core（AgentSession），把 wiki skills 注入系统提示词。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use mw_wiki::SkillLoader;
use r2_core::config::Config;
pub use r2_core::{AgentEvent, AgentSession};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// 网页可配置的 LLM 设置（持久化到 vault 根目录 appconfig.json 的 {"llm": {...}}）
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmConfig {
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl LlmConfig {
    /// 从环境变量读取：
    ///   MW_LLM_PROVIDER (openai_compat | anthropic，默认 openai_compat)
    ///   MW_LLM_BASE_URL / MW_LLM_API_KEY / MW_LLM_MODEL
    pub fn from_env() -> Self {
        Self {
            provider: std::env::var("MW_LLM_PROVIDER")
                .unwrap_or_else(|_| "openai_compat".into()),
            base_url: std::env::var("MW_LLM_BASE_URL").unwrap_or_default(),
            api_key: std::env::var("MW_LLM_API_KEY").unwrap_or_default(),
            model: std::env::var("MW_LLM_MODEL").unwrap_or_default(),
        }
    }

    /// appconfig.json 存在且含合法 "llm" 段则读它，否则回退环境变量
    pub fn load_or_env(path: &Path) -> Self {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(llm) = v.get("llm") {
                    if let Ok(cfg) = serde_json::from_value::<LlmConfig>(llm.clone()) {
                        return cfg;
                    }
                }
            }
        }
        Self::from_env()
    }

    /// 写 {"llm":{...}}，文件权限 600（含 api_key）
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let data = serde_json::json!({"llm": self});
        let text = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// 打码展示：`sk-…last4`（短 key 全打码）
    pub fn masked_key(&self) -> String {
        let k = &self.api_key;
        let chars: Vec<char> = k.chars().collect();
        if chars.is_empty() {
            return String::new();
        }
        if chars.len() <= 7 {
            return "****".to_string();
        }
        let head: String = chars[..3].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    }
}

pub struct WikiAgent {
    pub skills_root: PathBuf,
    pub work_dir: PathBuf,
    pub llm: LlmConfig,
    /// 人格层（mindrule.txt）：注入聊天 system_prompt 头部，让 Agent 以指定思维方式说话
    pub mindrule: Option<String>,
}

impl WikiAgent {
    /// LLM 配置取自环境变量（CLI ask / 兼容旧行为）
    pub fn new(skills_root: impl Into<PathBuf>, work_dir: impl Into<PathBuf>) -> Self {
        Self {
            skills_root: skills_root.into(),
            work_dir: work_dir.into(),
            llm: LlmConfig::from_env(),
            mindrule: None,
        }
    }

    /// 显式指定 LLM 配置（Web 热更新路径）
    pub fn with_llm(
        skills_root: impl Into<PathBuf>,
        work_dir: impl Into<PathBuf>,
        llm: LlmConfig,
    ) -> Self {
        Self {
            skills_root: skills_root.into(),
            work_dir: work_dir.into(),
            llm,
            mindrule: None,
        }
    }

    /// 设置人格层（链式）
    pub fn with_mindrule(mut self, mindrule: Option<String>) -> Self {
        self.mindrule = mindrule;
        self
    }

    /// 构造带 skills 的 Agent 系统提示词
    pub fn system_prompt(&self) -> Result<String> {
        let loader = SkillLoader::new(&self.skills_root);
        let mut prompt = String::from(
            "你是 Mind Wiki 的知识库引擎。严格遵守 skills 中的方法与铁律。\n\n\
             铁律：当前工作目录就是知识库根目录。所有文件读写、git 操作一律在当前目录内\
            （用相对路径）；绝不 cd 到其他目录，绝不读写当前目录之外的任何路径。\
             技能全文已注入本提示词，无需也不应去读技能文件本身。\n",
        );
        prompt.push_str(&loader.system_prompt_block()?);
        Ok(prompt)
    }

    /// 由 LlmConfig 构造 r2-core Config（base_url/model 为空时用 r2 默认）
    fn build_config(&self) -> Result<Config> {
        let mut config = Config::default_config();
        config.model.provider = self.llm.provider.clone();
        if self.llm.api_key.is_empty() {
            return Err(anyhow!("环境变量 MW_LLM_API_KEY 未设置"));
        }
        match config.model.provider.as_str() {
            "anthropic" => {
                if !self.llm.base_url.is_empty() {
                    config.model.anthropic.base_url = self.llm.base_url.clone();
                }
                config.model.anthropic.api_key = self.llm.api_key.clone();
                if !self.llm.model.is_empty() {
                    config.model.anthropic.model = self.llm.model.clone();
                }
            }
            "openai_compat" => {
                if !self.llm.base_url.is_empty() {
                    config.model.openai_compat.base_url = self.llm.base_url.clone();
                }
                config.model.openai_compat.api_key = self.llm.api_key.clone();
                if !self.llm.model.is_empty() {
                    config.model.openai_compat.model = self.llm.model.clone();
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

    /// 多轮聊天配置：人格层优先注入 + wiki skills + 多轮对话指引
    pub fn build_chat_config(&self) -> Result<Config> {
        let mut config = self.build_config()?;
        if let Some(rule) = self.mindrule.as_ref() {
            // 人格层放最前面：LLM 对 system_prompt 开头的指令锚定最强
            config.agent.system_prompt = format!(
                "{rule}\n\n===== 以下是你可调用的知识库操作技能（检索资料时用，但表达上必须内化为个人经验，绝不提及工具或知识库）=====\n{}\n\n这是多轮对话，用 wiki-query 技能查知识库回答。",
                config.agent.system_prompt
            );
        } else {
            config
                .agent
                .system_prompt
                .push_str("\n这是多轮对话，用 wiki-query 技能查知识库回答。\n");
        }
        Ok(config)
    }

    /// 真实跑一轮：创建 AgentSession（skills 系统提示词 + work_dir），prompt 一次，返回回答
    pub async fn ask(&self, question: &str) -> Result<String> {
        let config = self.build_config()?;
        let mut session = AgentSession::new(config).map_err(|e| anyhow!(e))?;
        session.prompt(question).await.map_err(|e| anyhow!(e))
    }

    /// 事件流版本：先建 AgentSession 拿 subscribe() receiver，再 spawn 跑 prompt，立即返回。
    /// 配置/会话创建失败时返回一个立即完成的 Err handle（receiver 直接关闭）。
    pub fn ask_with_events(
        &self,
        prompt: &str,
    ) -> (
        broadcast::Receiver<AgentEvent>,
        tokio::task::JoinHandle<Result<String, String>>,
    ) {
        let early_err = |msg: String| {
            let (_tx, rx) = broadcast::channel(1);
            (rx, tokio::spawn(async move { Err(msg) }))
        };
        let config = match self.build_config() {
            Ok(c) => c,
            Err(e) => return early_err(e.to_string()),
        };
        match AgentSession::new(config) {
            Ok(mut session) => {
                let rx = session.subscribe();
                let prompt = prompt.to_string();
                let handle = tokio::spawn(async move { session.prompt(&prompt).await });
                (rx, handle)
            }
            Err(e) => early_err(e),
        }
    }
}

/// 多轮聊天会话（包装 AgentSession，持有上下文）
pub struct ChatSession {
    inner: AgentSession,
}

impl ChatSession {
    pub fn new(cfg: Config) -> Result<Self, String> {
        Ok(Self {
            inner: AgentSession::new(cfg)?,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.inner.subscribe()
    }

    pub async fn send(&mut self, msg: &str) -> Result<String, String> {
        self.inner.prompt(msg).await
    }

    pub fn reset(&mut self) -> Result<(), String> {
        self.inner.reset_context();
        Ok(())
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

    #[test]
    fn llm_config_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("appconfig.json");
        let cfg = LlmConfig {
            provider: "openai_compat".into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            api_key: "sk-or-abcdef1234".into(),
            model: "deepseek/deepseek-v4-flash".into(),
        };
        cfg.save(&path).unwrap();

        // 权限 600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // save → load 一致
        let loaded = LlmConfig::load_or_env(&path);
        assert_eq!(loaded, cfg);

        // 打码：不含完整 key，保留尾 4 位
        let masked = cfg.masked_key();
        assert!(!masked.contains("abcdef"));
        assert!(masked.ends_with("1234"));

        // 文件不存在 → 回退 env（不 panic 即可）
        let _ = LlmConfig::load_or_env(&tmp.path().join("nope.json"));

        // 坏文件 → 回退 env（不 panic）
        let bad = tmp.path().join("bad.json");
        std::fs::write(&bad, "not json").unwrap();
        let _ = LlmConfig::load_or_env(&bad);
    }
}
