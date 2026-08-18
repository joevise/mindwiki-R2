# SPEC: Step 4 — Web 界面 + Agent 接加密会话完整闭环

## 目标
浏览器里走通全流程：创建库 → 解锁 → 上传 MD 入库（Agent）→ 聊天查询（Agent）→ 锁定。
前端用内嵌单页 HTML（Besure dashboard 风格，vanilla JS），React 前端迁移留到 Step 5。

## 改的文件
- `crates/mw-server/src/serve.rs` — 扩展成完整 Web 服务（内嵌前端 + vault 操作 API）
- `crates/mw-server/src/webui.html`（新）— 单页界面（include_str! 打进二进制）
- `crates/mw-server/src/main.rs` — serve 命令接完整服务
- `crates/mw-agent/src/lib.rs` — WikiAgent 增加 work_dir 参数化（指向解密会话的临时目录）
- `crates/mw-store/src/vault.rs` — 按需：查询型会话支持"只读模式"（不 seal）

## 详细设计

### 1. Web 服务 API（serve.rs 扩展）

现有：POST /api/gateway/open|close、GET /api/gateway/state

新增（都在已有 axum router 上）：

```
GET  /                     → 内嵌 webui.html
POST /api/vault/init       {password}                    → 在当前服务目录创建 vault（若已存在 409）
GET  /api/vault/status                                 → {exists, size, state: sealed|open}
POST /api/ingest           multipart file upload (.md)   → 入库流程（见下）
POST /api/query            {question}                    → 查询流程（见下）
```

**入库流程（ingest）：**
1. guard() 检查闸门开启（关闭则 423 Locked）
2. vault.open_session() 解密到临时目录
3. 上传的 .md 存到 work_dir 的待入库位置（如 inbox/upload-<ts>.md）
4. WikiAgent(work_dir=临时目录).ask(wiki-ingest 指令 + 文件路径)
5. vault.seal_session() 封印
6. 返回 agent 的回答摘要 + 生成文件列表

**查询流程（query）：**
1. guard() 检查闸门
2. open_session 解密
3. WikiAgent.ask(wiki-query 指令 + 问题)
4. 默认只读：agent 回答后直接 drop session（不 seal，容器保持原状）
   —— wiki-query skill 本身允许用户选择沉淀答案；V1 里 seal 与否由 agent 是否在 wiki/ 产生了新文件决定（简单判断：对比 work_dir 文件哈希或 git status，有变更才 seal）
5. 返回答案

**LLM 配置**：serve 启动时从环境变量 MW_LLM_* 读（与 ask 命令一致）；未配置则 ingest/query 返回 503 + 明确提示。

**并发**：
- 每 vault 一把 tokio Mutex（同时只允许一个解密会话在工作——加密容器的天然约束）
- 用 Arc<Mutex<()>> 简单实现，注释标注后续可按 vault_id 建锁表

### 2. 前端（webui.html）

单页 vanilla JS（白底黑字红点缀，沿用 PPT 视觉）：

```
┌────────────────────────────────────┐
│ Mind Wiki                          │
├────────────────────────────────────┤
│ 状态栏：密封 🔒 / 已解锁 🔓        │
├────────────────────────────────────┤
│ [密封状态]                          │
│   密码输入框 + [解锁]按钮           │
│   或 [创建知识库]（不存在时）        │
├────────────────────────────────────┤
│ [解锁状态]                          │
│   📤 上传 Markdown 入库            │
│   💬 查询输入框 + 回答区           │
│   🔒 [一键锁定]按钮（醒目红色）     │
│   Agent 工作中显示 spinner         │
└────────────────────────────────────┘
```

JS 调 API，轮询/等待 fetch 完成。错误友好提示（闸门关闭→"知识库已锁定"）。

### 3. WikiAgent 修改（mw-agent）

```rust
pub struct WikiAgent {
    pub skills_root: PathBuf,
    pub work_dir: PathBuf,   // 指向解密会话临时目录
}
impl WikiAgent {
    pub fn new(skills_root, work_dir) -> Self;  // 已差不多，确认 work_dir 真用上（config.agent.work_dir）
    pub async fn ask(&self, prompt: &str) -> Result<String>;  // 已有
}
```

确认 build_config() 里 work_dir 用 self.work_dir 而不是当前目录（Step1 里可能写死了 cwd）。

### 4. CLI serve 命令

```bash
mindwiki serve [--port 7900] [--skills ./skills]
# 启动信息打印：Web 界面 http://127.0.0.1:7900
```

serve 同时挂载 Step 3 的 gateway 端点（不破坏现有测试）。

### 5. 测试

```rust
#[tokio::test] async fn full_loop_e2e() {
    // 起服务（随机端口）
    // POST /api/vault/init (password=test) → 200
    // POST /api/gateway/open → 200
    // GET /api/vault/status → open
    // POST /api/gateway/close → 200
    // POST /api/query (未配 LLM) → 503 带提示
    // POST /api/query (闸门关闭) → 423
}
// 不依赖真实 LLM 的部分全部自动化；
// LLM 集成手动验收（README 里写清楚怎么跑）
```

## 验收
1. cargo build --release + cargo test --workspace 全绿（新增 ≥2 测试）
2. mindwiki serve 起来后浏览器访问完整流程（无 LLM 时给出清晰 503 提示）
3. 手动 E2E（配 MW_LLM_*）：init → unlock → ingest demo/火炬电子管理决策案例.md → query "这个案例的核心决策方法是什么" → lock
4. 提交推送
