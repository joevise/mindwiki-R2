//! # mw-wiki — L2 知识库引擎
//!
//! wiki init/ingest/query/lint 的领域逻辑 + skills 加载器。
//! 只依赖 L1 的 SecretStore trait —— 可独立更新，不动加密层。

pub mod skills;

pub use skills::SkillLoader;
