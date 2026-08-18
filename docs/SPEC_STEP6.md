# SPEC: Step 6 — 聊天主界面 + 模型设置页 + 入库实时进度 + RAW 预览

## 目标
从"工具页"进化成"产品"：聊天为主界面（多轮会话），模型可网页配置（provider/base_url/key/model，热生效），入库过程 SSE 实时展示，原始文件可点击预览。

## 改的文件
- `crates/mw-agent/src/lib.rs` — LlmConfig 文件化 + ask_with_events + ChatSession 多轮
- `crates/mw-server/src/serve.rs` — 设置 API + SSE ingest + chat API + 状态接线
- `crates/mw-server/src/webui.html` — 聊天主界面改版

## 详细设计

### 1. mw-agent

```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct LlmConfig { pub provider: String, pub base_url: String, pub api_key: String, pub model: String }
impl LlmConfig {
    pub fn from_env() -> Self;                       // 现有 env 逻辑抽出来
    pub fn load_or_env(path: &Path) -> Self;         // appconfig.json 存在则读它，否则 env
    pub fn save(&self, path: &Path) -> Result<(),String>;  // 写 {"llm":{...}}，权限 600
}

// WikiAgent 增加事件流版本（ingest 用）
pub fn ask_with_events(&self, prompt:&str)
    -> (tokio::sync::broadcast::Receiver<r2_core::AgentEvent>,
        tokio::task::JoinHandle<Result<String,String>>);
// 实现：先建 AgentSession，拿 subscribe() receiver，spawn task 里跑 prompt，立即返回

// 多轮聊天会话（chat 用）
pub struct ChatSession { inner: r2_core::AgentSession }
impl ChatSession {
    pub fn new(cfg: Config) -> Result<Self,String>;
    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent>;
    pub async fn send(&mut self, msg:&str) -> Result<String,String>;  // prompt 包装
    pub fn reset(&mut self) -> Result<(),String>;                     // reset_context
}
```
chat 的 system_prompt：与 wiki skills 相同注入（Agent 能查 wiki），追加一句"这是多轮对话，用 wiki-query 技能查知识库回答"。

### 2. serve.rs

AppState 增：
```rust
llm: RwLock<LlmConfig>,                       // 启动 load_or_env(appconfig.json)，热更新
chat: tokio::sync::Mutex<Option<mw_agent::ChatSession>>,  // 解锁后懒建，锁定时清空
```

新 API（全部闸门内 423）：
```
GET  /api/llm/config   → {provider, base_url, model, api_key_masked}   // key 打码 sk-…last4
POST /api/llm/config   {provider, base_url, api_key, model}
     → 校验非空 → save appconfig.json → *llm 写改 → chat 清空（下轮用新模型）→ 200
POST /api/ingest/stream  multipart .md → 响应 text/event-stream：
     data: {"type":"tool_call","name":"read","detail":"inbox/xx.md"}
     data: {"type":"message","text":"…增量…"}
     data: {"type":"done","answer":"…","files":[...]}   // 最后一条，files 可点击
     （AgentEvent→SSE 映射：ToolCall→tool_call、MessageUpdate→message、Done→done、Error→error、其余跳过；
       ToolCall 的 detail 截 arguments 前 80 字符）
POST /api/chat  {message} → SSE 同上格式（message/done）；多轮：chat session 懒建复用
POST /api/chat/reset → 清空 chat 会话
```
旧 POST /api/ingest 保留（非流式，兼容测试）。gateway close 处理里加：chat 清空。

LLM key 检查改为用 AppState.llm（不再直接 env）。

### 3. webui.html 改版（聊天为主）

布局（解锁后）：
```
┌──────────┬──────────────────────────────┐
│ 侧栏(可折叠)│  聊天区                        │
│ [上传入库] │  ┌────────────────────────┐  │
│ [文件树]  │  │ 消息流（user 右/agent 左）│  │
│ [图谱]   │  │ agent 消息内嵌"工作中"状态行│  │
│ [设置⚙️] │  └────────────────────────┘  │
│         │  [输入框………………] [发送] [新对话] │
└──────────┴──────────────────────────────┘
```
- 顶部按钮折叠侧栏；主区就是聊天
- 消息流：user 气泡右、agent 左；agent 回答走现有 markdown 渲染器（wikilink 可点）
- 发送中：输入框禁用 + agent 气泡内显示实时状态行（tool_call 摘要滚动，最多显示最近 3 条，灰色小字）
- 入库（侧栏）：上传后进度用同样的状态行样式，逐条 append；完成的 files 列表每项可点 → 右侧弹层或聊天区插入该文件预览（用 /api/wiki/page 渲染）
- 设置（侧栏卡片）：provider 下拉（openai_compat / anthropic）+ 「OpenRouter 预设」按钮（自动填 base_url=https://openrouter.ai/api/v1）、base_url、api_key、model 四个字段 + 保存 → POST 后提示"已生效"
- SSE 读取用 fetch POST + ReadableStream 手动解析 `data: ` 行（EventSource 不支持 POST）
- 锁定时：一切回锁定页（现有逻辑）

### 4. 测试
```rust
llm_config_roundtrip          // save→load 一致，文件权限 600
llm_config_endpoint_gated     // 未解锁 GET /api/llm/config → 423；解锁后 GET 返回 masked key
ingest_stream_endpoint        // 起服务（假 key）→ SSE 响应 content-type 正确、以 done/error 事件结束
chat_requires_unlock          // 423
chat_reset_clears             // reset 后 chat=None（内部状态断言或二次行为）
```
既有 25 测试不破坏（/api/ingest 旧端点保留）。

## 验收
1. cargo build --release + cargo test --workspace 全绿（新增 ≥5）
2. 手动 E2E：解锁 → 设置里配 OpenRouter（deepseek/deepseek-v4-flash）→ 不重启直接聊天能用 → 入库流式看进度 → files 点击预览 → 锁定全清
3. 提交推送
