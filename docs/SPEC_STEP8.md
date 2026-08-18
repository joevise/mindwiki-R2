# SPEC: Step 8 — 删除功能（快速 + 智能）+ 乱码修复

## 目标
知识库支持删除：快速删除（垃圾/普通文件）+ 智能删除（知识页面，Agent 清理引用保持一致）。删除后 git 提交（可回滚）、图谱自动更新。顺手清理演示库的乱码目录。

## 改的文件
- `crates/mw-server/src/serve.rs` — DELETE /api/wiki/entry + smart-delete 流程
- `crates/mw-server/src/webui.html` — 文件树删除按钮 + 确认弹层 + 智能删除进度

## 详细设计

### 1. API：DELETE /api/wiki/entry

```json
请求：{ "path": "wiki/xxx.md", "mode": "quick" | "smart" }
响应 quick：{ "deleted": true, "files_removed": N }
响应 smart：{ "deleted": true, "files_removed": N, "answer": "Agent 清理报告", "files_touched": [...] }
```

共同逻辑：
- 路径 sanitize（同 /api/wiki/page 的 canonicalize 校验，越界 400）
- 保护名单：`index.md`、`schema.md`、`log.md` 不可删（400 + 说明）
- 闸门关闭 423；走 vault_lock；用长驻会话的 work_dir

### 2. 快速删除（mode=quick）
1. 删除目标（文件或目录，目录整树删）
2. `git add -A && git commit -m "Delete: {path}"`
3. seal + 重建会话（同 import 的做法）
4. 返回 files_removed 计数

### 3. 智能删除（mode=smart）
1. 快速删除的 1-2 步先执行（先删掉）
2. WikiAgent.ask(删除后清理指令)：
   - prompt：文件 {path} 已删除。请扫描全库（grep 搜索 [[basename]] 引用），清理所有悬空链接（直接移除该 wikilink 或标注"已删除"），更新 index.md 检索路由（如有该页条目），如 log.md 需要追加删除记录则追加。完成后简述清理了哪些文件。不要向用户提问。
   - Agent 只做引用清理，不重新萃取
3. snapshot diff → files_touched
4. seal + 重建会话
5. 返回 Agent 清理报告

注意：smart 模式无 LLM 配置 → 降级为 quick 并在响应里注明 "degraded": true。

### 4. 前端（webui.html）
文件树每个节点（文件+目录）右侧悬停显示 🗑 图标：
- 点击 → 确认弹层：显示路径 + 两个选项
  - 「快速删除」：普通文件/垃圾目录用，立即执行
  - 「智能删除」：Agent 清理引用，显示 spinner + 实时状态行（复用 ingest 的 SSE 样式——本版先不做 SSE，用普通 POST + 全局 overlay spinner 也可接受）
- 保护名单文件不显示删除按钮（或点击提示不可删）
- 删除完成 → 自动刷新 loadTree + loadGraph，toast 提示删除结果
- 确认弹层文案要吓人一点：「删除 {path}？（历史版本仍保留，可恢复）」

### 5. 测试
```rust
#[tokio::test] async fn delete_quick_removes_and_commits()
// init+open → 写几个文件 commit → DELETE quick → tree 无此文件 → close→reopen 数据还是删了 → git log 有 Delete commit
#[tokio::test] async fn delete_protected_paths_rejected()   // index.md/schema.md/log.md → 400
#[tokio::test] async fn delete_requires_unlock()            // 423
#[tokio::test] async fn delete_smart_degrades_without_llm() // 无 LLM 配置 → quick 结果 + degraded:true
#[tokio::test] async fn delete_path_traversal_blocked()     // ../../etc → 400
```

### 6. 演示库乱码清理（部署后手动执行）
用新端点删掉 `019f02e3-13a8-7e1c-af7c-877874ae1f6e`（quick 模式，163 文件）。
另外库里 `reports/`（乱码 html）也是原 zip 直接上传的重复品 → 一并 quick 删除。
删后验证：图谱回到正常（顶层 wiki/ 干净版还在）。

## 验收
1. cargo build --release + cargo test --workspace 全绿（新增 ≥5 测试，33 旧不破坏）
2. E2E：quick 删文件 → 树消失 → 锁定重开确认已删 → git log 有 Delete 提交；smart 删 wiki 页面 → 引用被清理（grep 不到悬空链接）
3. 演示库乱码目录清干净，图谱正常
4. 提交推送（message: Step8: 删除功能（快速+智能）+ 引用清理）
