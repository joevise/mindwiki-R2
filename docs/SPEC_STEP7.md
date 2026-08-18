# SPEC: Step 7 — 现成 Wiki 整包导入（结构保留）

## 目标
用户把现成的 Wiki/Obsidian Vault（md 文件集合，可能带 [[wikilink]]）打包成 zip 上传，系统原样导入：文件结构保留、不重新萃取、导入后文件树/图谱/聊天立即可用。与现有"AI 萃取入库"（/api/ingest）互补。

## 改的文件
- `crates/mw-server/Cargo.toml` — 加 `zip = "2"` 依赖
- `crates/mw-server/src/serve.rs` — 新增 POST /api/wiki/import
- `crates/mw-server/src/webui.html` — 上传卡片加"导入 Wiki 压缩包"入口

## 详细设计

### 1. API

```
POST /api/wiki/import   multipart（字段 file = 一个 .zip）
闸门关闭 → 423；无 LLM 配置不影响（本端点不调 LLM）
```

处理流程：
1. 读 multipart 拿 zip 字节（上限 50MB，超出 413）
2. 内存解包（zip crate）：
   - 逐条 entry：路径 sanitize——拒绝 `..`、绝对路径、符号链接 entry（返回 400 带恶意路径名）
   - 跳过垃圾：`.git/`、`.obsidian/`、`.trash/`、`__MACOSX/`、`.DS_Store`、`Thumbs.db`
   - 只接受文件（目录 entry 自动创建）
3. 长驻会话（s.current_session）里写入 work_dir：
   - 冲突策略：同名文件覆盖（导入是显式用户动作）；先收集 imported/skipped 清单
4. `session.git_commit("Import wiki bundle: N files")`
5. `vault.seal_session()` 后**重建会话**（seal 会话销毁，需要 open_session 新建放回 AppState——参考 ingest_handler 现有做法，保持一致）
6. 返回 `{imported: [...], skipped: [...], committed: true}`

注意：与 ingest/query 一样走 `s.vault_lock` 串行；session 不存在（未解锁）→ 423。

### 2. 前端（webui.html）

上传入库卡片里加第二个入口：
- 「📤 上传文档」现有（AI 萃取）
- 「📦 导入 Wiki 压缩包」新：accept=".zip"，上传中 spinner，完成后显示 imported 数量 + 跳过列表（折叠），提示"文件树/图谱已更新"
- 文件树/图谱在导入完成后自动刷新（调用现有 refresh/loadTree/loadGraph）

### 3. 测试

```rust
#[tokio::test] async fn import_zip_roundtrip()
// 造 zip（wiki/A.md 带 [[B]]、sources/x.md、.git/junk、.obsidian/config）
// init+open → import → tree 含 A.md/x.md 不含 .git/.obsidian → graph 有 A→B 边（若 B 也导入）→ close 后 reopen 数据还在
#[tokio::test] async fn import_rejects_path_traversal()
// zip 里造 entry "../../etc/evil" → 400
#[tokio::test] async fn import_requires_unlock()   // 423
```

zip 构造测试用 `zip::ZipWriter` 写内存。

## 验收
1. cargo build --release + cargo test --workspace 全绿（新增 ≥3 测试，30 旧不破坏）
2. 手动 E2E：serve → 解锁 → curl 导入一个真实 zip（含中文文件名 UTF-8 flag）→ 文件树/图谱立即可见 → 锁定重开数据在
3. 提交推送（message: Step7: 现成 Wiki 整包导入（结构保留））
