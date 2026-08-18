# SPEC: Step 5 — Wiki 浏览器（文件树/页面/图谱）

## 目标
用户在 Web 界面里能看见自己的知识库：文件树浏览、页面内容阅读、Obsidian 式图谱视图（节点=页面、连线=wikilink、按类型着色、节点大小=连接数）。

## 改的文件
- `crates/mw-server/src/serve.rs` — 长驻解密会话 + 3 个新 API
- `crates/mw-server/src/webui.html` — 三标签视图（树/页面/图谱）
- `crates/mw-store/src/vault.rs` — 如需：Session 复用相关小改

## 核心架构变化：长驻解密会话

现状：每次 ingest/query 都 open_session→seal，重复 tar/untar。
新模型：**解锁即解密，锁定即销毁**——

```
POST /api/gateway/open → 验证密码 → vault.open_session() → 存入 AppState.current_session
后续所有操作（ingest/query/browse）都复用这个 session 的 work_dir
ingest 完成后 seal_session()（更新容器）但 session 保持存活
POST /api/gateway/close → 终止 + 销毁 session（不 seal 未提交变更——ingest 已即时 seal）
```

AppState 加：`current_session: tokio::sync::RwLock<Option<mw_store::DecryptedSession>>`
per-vault Mutex 保留（防并发写）。注意 DecryptedSession 里 Agent work_dir 指向它。

## 三个新 API

```
GET /api/wiki/tree
→ 遍历 session.work_dir，返回 JSON 树：
  {name, path, type:"dir"|"file", children:[...]}
  排除 .git、.gitkeep；目录排序在前

GET /api/wiki/page?path=wiki/xxx.md
→ 返回 {path, content}（原文 markdown）
→ 路径必须 work_dir 内（防 ../ 穿越：canonicalize 后检查前缀）

GET /api/wiki/graph
→ 遍历 wiki/**/*.md + index.md：
  nodes: [{id: "wiki/概念A.md", label: "概念A", type: "concept"}]
    type 从 frontmatter `type:` 行解析（无则 "page"；index.md 为 "index"）
  edges: [{from, to}] — 解析正文所有 [[wikilink]] / [[path#anchor|text]]，
    链接目标是 basename（如 [[source-record-abc--1a2b3c4d#Evidence|x]] → 找同名文件）
    找不到目标的悬空链接跳过
→ 全部在 session.work_dir 上操作，只读
→ 闸门关闭 → 423
```

## 前端（webui.html）

顶部加三个标签：**文件树 | 图谱**（页面浏览融入：点树/图谱节点显示页面）。保持白底黑字红点缀。

布局：
```
解锁后的主区域改为：
┌─────────────┬────────────────────────┐
│ 标签: [文件树][图谱]                     │
│ ┌─────────┐ │  页面内容区（渲染 md）    │
│ │ 文件树   │ │                        │
│ │ 或图谱   │ │                        │
│ └─────────┘ │                        │
└─────────────┴────────────────────────┘
```

**文件树**：嵌套 ul/li，目录可折叠，点文件 → GET /api/wiki/page → 右侧渲染。
**Markdown 渲染**：写一个轻量渲染器（~80 行）：# 标题、**粗体**、`代码`、``` 代码块、- 列表、> 引用、[[wikilink]] 渲染成蓝色可点（点击加载目标页面）。不引库。
**图谱视图**（核心，~200 行 canvas）：
- GET /api/wiki/graph → nodes/edges
- 力导向布局：斥力（节点间）+ 引力（边）+ 中心重力，requestAnimationFrame 迭代 300 帧后静止
- 节点半径 = 6 + 连接数 × 2
- 按 type 着色：index=#1B1D22 黑、concept=#F5453D 品牌红、algorithm=#E0941F 琥珀、paper=#3B82F6 蓝、experiment=#0E9C8E 青、source-record=#9AA0AA 灰、page=默认 #6B7280
- 悬停：高亮节点+相邻边/节点，其他半透明
- 点击节点：加载该页面到右侧
- 拖拽节点（mousedown+mousemove 改坐标，标记 fixed）、滚轮缩放、空白处拖平移
- 图例（左下角小字列出颜色含义）

锁定后：清空树/图谱/页面区，回到锁定界面。

## 测试

```rust
#[test] fn graph_parsing()  // 造几个 md 文件带 [[links]]，验证 nodes/edges/type 解析
#[test] fn tree_excludes_git()  // .git 不出现
#[test] fn page_path_traversal_blocked()  // path=../etc/passwd → 400
#[tokio::test] async fn browse_requires_unlock()  // 未解锁 GET /api/wiki/tree → 423
#[tokio::test] async fn open_creates_session_close_destroys()  // 长驻会话生命周期
```

## 验收
1. cargo build --release + cargo test --workspace 全绿（新增 ≥4 测试）
2. 手动 E2E：serve → 解锁 → ingest 火炬电子案例 → 文件树可见 → 点页面可读 → 图谱有节点和连线、颜色/大小正确、悬停高亮、点击跳页面 → 锁定后界面回锁定态
3. 提交推送
