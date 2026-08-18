# SPEC: Step 1 — r2-core AgentSession 真实跑通 wiki skills

## 目标
mw-agent 里嵌入 r2-core 的 AgentSession，用真实 LLM 跑通一次 wiki-init（在一个空目录初始化一个最小 Wiki）。

## 改的文件
- `crates/mw-agent/src/lib.rs` — 真实 AgentSession 集成
- `crates/mw-agent/Cargo.toml` — 如需加依赖
- `crates/mw-server/src/main.rs` — 加 `mindwiki ask <prompt>` 命令（走 Agent）
- 可能需要 r2-core 的 config 结构支持从环境变量读 API key

## 详细设计

### 1. WikiAgent 真实实现（mw-agent/src/lib.rs）

```rust
use r2_core::{AgentSession, AgentEvent, config::Config};

pub struct WikiAgent {
    skills_root: PathBuf,
    work_dir: PathBuf,   // wiki 根目录（Agent 工作目录）
}

impl WikiAgent {
    pub fn new(skills_root, work_dir) -> Self;

    /// 构造系统提示词（已有，保留）
    pub fn system_prompt(&self) -> Result<String>;

    /// 真实跑一轮：创建 AgentSession（带 skills 系统提示词 + work_dir），
    /// prompt 一次，返回回答文本
    pub async fn ask(&self, question: &str) -> Result<String> {
        let mut config = Config::default_config();
        // 从环境变量读 LLM 配置：
        //   MW_LLM_PROVIDER (openai_compat | anthropic，默认 openai_compat)
        //   MW_LLM_BASE_URL (默认 https://api.minimaxi.com/anthropic/v1 或空用 r2 默认)
        //   MW_LLM_API_KEY  (必须)
        //   MW_LLM_MODEL    (默认 k3 或 r2 默认)
        // 设置 config 的 model 段 + work_dir
        let mut session = AgentSession::new(config)?;
        // 系统提示词注入方式：看 r2-core Config 是否有 system_prompt 字段；
        // 若无，把 skills block 拼在用户 prompt 前面（前置一次指令也可以）
        let reply = session.prompt(&format!("{}\n\n---\n用户指令：{}", skills_block, question)).await?;
        Ok(reply)
    }
}
```

**关键**：先读 r2-core 的 `config.rs` 和 `session_api.rs`，确认：
- Config 结构怎么设 model provider/base_url/api_key/model
- AgentSession::new 是否接受 work_dir（r2 CLI 有 --work-dir，说明 Config 里有）
- 有没有 system_prompt 配置字段

### 2. CLI 命令（mw-server/src/main.rs）

```rust
Some("ask") => {
    // mindwiki ask "在当前目录初始化一个测试 Wiki，主题：AI 记忆系统"
    // 从 env 读 MW_LLM_*，调 WikiAgent::ask，打印回答
}
Some("init-wiki") => {
    // 便捷命令：预置 prompt 调 wiki-init skill
    // mindwiki init-wiki <dir> "主题描述"
}
```

### 3. 测试验证（不写自动化测试，手动端到端）

```bash
export MW_LLM_API_KEY=sk-...  # 用 kimi key
mkdir -p /tmp/wiki-test && cd /tmp/wiki-test
mindwiki ask "按 wiki-init skill 的 Host Context 模式初始化 Wiki：
host_id=test, name=测试, background=AI公司, goal=验证, expertise=Rust,
build_rule=技术文档为主。主题：AI 记忆系统"
# 验证生成：index.md log.md schema.md sources/ wiki/
```

LLM 用 kimi coding 端点（TOOLS.md 里有现成 key）：
- base_url: https://api.minimaxi.com/anthropic/v1 （r2 支持 anthropic 协议）
- 或 openai 兼容端点

### 4. 不要做的
- 不做加密/解密（Step 2 的事）
- 不做 Web 服务（Step 4 的事）
- 不改 r2-agent 仓库（如果 r2-core 缺配置字段，在本仓库包一层适配，宁可包适配也不 fork）

## 验收
1. `cargo build --release` 通过
2. `mindwiki ask "你好，你能看到哪些 skills？"` 返回包含 wiki-init/ingest/query/lint 的回答
3. 端到端：mindwiki ask 跑 wiki-init，在空目录生成 index.md/schema.md/log.md/sources/wiki/ 结构
4. 全部提交推送
